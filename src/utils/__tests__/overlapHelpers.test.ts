import { describe, it, expect } from 'vitest';
import {
  needsDirectionReversal,
  computeOverlapIds,
  computeActionableIds,
  computeTargetShotIndex,
  computeMergeOptions,
  defaultMergeOffset,
  reverseTargetShot,
  applyTargetData,
  interpolateShots,
  avgNullable,
  type TargetShotLike,
  type EditableShotLike,
  type OverlapParams,
} from '../overlapHelpers';

// ─── Test data helpers ──────────────────────────────────────────────────────

/** Create a realistic cave survey shot heading roughly NE at ~45°. */
function makeTargetShot(overrides: Partial<TargetShotLike> = {}): TargetShotLike {
  return {
    azimuth_in: 45,
    azimuth_out: 46,
    length: 5.0,
    depth_in: 10,
    depth_out: 12,
    pitch_in: -5,
    pitch_out: -6,
    left: 2.0,
    right: 3.0,
    up: 1.5,
    down: 0.5,
    ...overrides,
  };
}

function makeEditableShot(id: number, overrides: Partial<EditableShotLike> = {}): EditableShotLike {
  return {
    id,
    azimuth_in: 90,
    azimuth_out: 91,
    length: 4.0,
    depth_in: 8,
    depth_out: 10,
    pitch_in: -3,
    pitch_out: -4,
    left: 1.5,
    right: 2.5,
    up: 1.0,
    down: 0.8,
    comment: null,
    kind: 'Normal' as const,
    ...overrides,
  };
}

// ─── needsDirectionReversal ─────────────────────────────────────────────────

describe('needsDirectionReversal', () => {
  it('Front + forward → false', () => {
    expect(needsDirectionReversal(0, 'forward')).toBe(false);
  });

  it('Front + reverse → true', () => {
    expect(needsDirectionReversal(0, 'reverse')).toBe(true);
  });

  it('Back + forward → true', () => {
    expect(needsDirectionReversal(5, 'forward')).toBe(true);
  });

  it('Back + reverse → false', () => {
    expect(needsDirectionReversal(5, 'reverse')).toBe(false);
  });

  it('Back with any station > 0 + forward → true', () => {
    expect(needsDirectionReversal(1, 'forward')).toBe(true);
    expect(needsDirectionReversal(10, 'forward')).toBe(true);
  });

  it('Back with any station > 0 + reverse → false', () => {
    expect(needsDirectionReversal(1, 'reverse')).toBe(false);
    expect(needsDirectionReversal(10, 'reverse')).toBe(false);
  });
});

// ─── computeOverlapIds ──────────────────────────────────────────────────────

describe('computeOverlapIds', () => {
  it('Front: returns 0..matchedShots-1', () => {
    const ids = computeOverlapIds(0, 3);
    expect(ids).toEqual(new Set([0, 1, 2]));
  });

  it('Front: single matched shot', () => {
    const ids = computeOverlapIds(0, 1);
    expect(ids).toEqual(new Set([0]));
  });

  it('Back: returns (importStationIndex-matchedShots)..(importStationIndex-1)', () => {
    // importStationIndex=5, matchedShots=3 → shots 2,3,4
    const ids = computeOverlapIds(5, 3);
    expect(ids).toEqual(new Set([2, 3, 4]));
  });

  it('Back: single matched shot', () => {
    // importStationIndex=5, matchedShots=1 → shot 4
    const ids = computeOverlapIds(5, 1);
    expect(ids).toEqual(new Set([4]));
  });

  it('Back: matchedShots === importStationIndex (full overlap)', () => {
    // importStationIndex=3, matchedShots=3 → shots 0,1,2
    const ids = computeOverlapIds(3, 3);
    expect(ids).toEqual(new Set([0, 1, 2]));
  });

  it('returns empty set for 0 matched shots', () => {
    expect(computeOverlapIds(0, 0)).toEqual(new Set());
    expect(computeOverlapIds(5, 0)).toEqual(new Set());
  });
});

// ─── computeActionableIds ───────────────────────────────────────────────────

describe('computeActionableIds', () => {
  it('Front: shots 0..mergeImportStation-1 are actionable', () => {
    // importStationIndex=0, matchedShots=3, mergeImportStation=2
    // overlap = {0,1,2}, actionable = shots 0..1 = {0,1}
    const ids = computeActionableIds(0, 3, 2);
    expect(ids).toEqual(new Set([0, 1]));
  });

  it('Front: mergeImportStation=0 → nothing actionable', () => {
    const ids = computeActionableIds(0, 3, 0);
    expect(ids).toEqual(new Set());
  });

  it('Front: mergeImportStation=matchedShots → all overlap actionable', () => {
    const ids = computeActionableIds(0, 3, 3);
    expect(ids).toEqual(new Set([0, 1, 2]));
  });

  it('Back: shots mergeImportStation..(importStationIndex-1) are actionable', () => {
    // importStationIndex=5, matchedShots=3, mergeImportStation=3
    // overlap = {2,3,4}, actionable = shots 3..4 = {3,4}
    const ids = computeActionableIds(5, 3, 3);
    expect(ids).toEqual(new Set([3, 4]));
  });

  it('Back: mergeImportStation=importStationIndex → nothing actionable', () => {
    // mergeImportStation=5, range is 5..4 which is empty
    const ids = computeActionableIds(5, 3, 5);
    expect(ids).toEqual(new Set());
  });

  it('Back: mergeImportStation at overlap start → all overlap actionable', () => {
    // importStationIndex=5, matchedShots=3, mergeImportStation=2
    // overlap = {2,3,4}, actionable = shots 2..4 = {2,3,4}
    const ids = computeActionableIds(5, 3, 2);
    expect(ids).toEqual(new Set([2, 3, 4]));
  });
});

// ─── computeTargetShotIndex ─────────────────────────────────────────────────

describe('computeTargetShotIndex', () => {
  // Example: 3 matched shots, existingStationIndex=10

  describe('Front + forward', () => {
    const params: OverlapParams = {
      importStationIndex: 0,
      matchedShots: 3,
      existingStationIndex: 10,
      direction: 'forward',
    };

    it('overlapId=0 → target 10', () => {
      expect(computeTargetShotIndex(0, params)).toBe(10);
    });

    it('overlapId=1 → target 11', () => {
      expect(computeTargetShotIndex(1, params)).toBe(11);
    });

    it('overlapId=2 → target 12', () => {
      expect(computeTargetShotIndex(2, params)).toBe(12);
    });
  });

  describe('Front + reverse', () => {
    const params: OverlapParams = {
      importStationIndex: 0,
      matchedShots: 3,
      existingStationIndex: 10,
      direction: 'reverse',
    };

    it('overlapId=0 → target 9', () => {
      expect(computeTargetShotIndex(0, params)).toBe(9);
    });

    it('overlapId=1 → target 8', () => {
      expect(computeTargetShotIndex(1, params)).toBe(8);
    });

    it('overlapId=2 → target 7', () => {
      expect(computeTargetShotIndex(2, params)).toBe(7);
    });
  });

  describe('Back + forward', () => {
    // importStationIndex=5, matchedShots=3 → overlapStart=2
    const params: OverlapParams = {
      importStationIndex: 5,
      matchedShots: 3,
      existingStationIndex: 10,
      direction: 'forward',
    };

    it('overlapId=2 (k=0) → target 12', () => {
      expect(computeTargetShotIndex(2, params)).toBe(12);
    });

    it('overlapId=3 (k=1) → target 11', () => {
      expect(computeTargetShotIndex(3, params)).toBe(11);
    });

    it('overlapId=4 (k=2) → target 10', () => {
      expect(computeTargetShotIndex(4, params)).toBe(10);
    });
  });

  describe('Back + reverse', () => {
    // importStationIndex=5, matchedShots=3 → overlapStart=2
    const params: OverlapParams = {
      importStationIndex: 5,
      matchedShots: 3,
      existingStationIndex: 10,
      direction: 'reverse',
    };

    it('overlapId=2 (k=0) → target 7', () => {
      expect(computeTargetShotIndex(2, params)).toBe(7);
    });

    it('overlapId=3 (k=1) → target 8', () => {
      expect(computeTargetShotIndex(3, params)).toBe(8);
    });

    it('overlapId=4 (k=2) → target 9', () => {
      expect(computeTargetShotIndex(4, params)).toBe(9);
    });
  });

  it('handles existingStationIndex=0 with Front+forward', () => {
    const params: OverlapParams = {
      importStationIndex: 0,
      matchedShots: 2,
      existingStationIndex: 0,
      direction: 'forward',
    };
    expect(computeTargetShotIndex(0, params)).toBe(0);
    expect(computeTargetShotIndex(1, params)).toBe(1);
  });
});

// ─── computeMergeOptions ────────────────────────────────────────────────────

describe('computeMergeOptions', () => {
  describe('Front + forward (non-partial)', () => {
    const suggestion = {
      importStationIndex: 0,
      existingStationIndex: 10,
      matchedShots: 3,
      direction: 'forward' as const,
      isPartial: false,
    };

    it('produces matchedShots+1 options', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts).toHaveLength(4); // k=0,1,2,3
    });

    it('first option: import=0, existing=10', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts[0]).toEqual({ offset: 0, importStation: 0, existingStation: 10 });
    });

    it('last option: import=3, existing=13', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts[3]).toEqual({ offset: 3, importStation: 3, existingStation: 13 });
    });

    it('import station increases, existing station increases', () => {
      const opts = computeMergeOptions(suggestion, 8);
      for (let i = 1; i < opts.length; i++) {
        expect(opts[i]!.importStation).toBeGreaterThan(opts[i - 1]!.importStation);
        expect(opts[i]!.existingStation).toBeGreaterThan(opts[i - 1]!.existingStation);
      }
    });
  });

  describe('Front + reverse (non-partial)', () => {
    const suggestion = {
      importStationIndex: 0,
      existingStationIndex: 10,
      matchedShots: 3,
      direction: 'reverse' as const,
      isPartial: false,
    };

    it('produces matchedShots+1 options', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts).toHaveLength(4);
    });

    it('first option: import=0, existing=10', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts[0]).toEqual({ offset: 0, importStation: 0, existingStation: 10 });
    });

    it('last option: import=3, existing=7', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts[3]).toEqual({ offset: 3, importStation: 3, existingStation: 7 });
    });

    it('import station increases, existing station decreases', () => {
      const opts = computeMergeOptions(suggestion, 8);
      for (let i = 1; i < opts.length; i++) {
        expect(opts[i]!.importStation).toBeGreaterThan(opts[i - 1]!.importStation);
        expect(opts[i]!.existingStation).toBeLessThan(opts[i - 1]!.existingStation);
      }
    });
  });

  describe('Back + forward (non-partial)', () => {
    const suggestion = {
      importStationIndex: 5,
      existingStationIndex: 10,
      matchedShots: 3,
      direction: 'forward' as const,
      isPartial: false,
    };

    it('produces matchedShots+1 options', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts).toHaveLength(4);
    });

    it('first option: import=5, existing=10', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts[0]).toEqual({ offset: 0, importStation: 5, existingStation: 10 });
    });

    it('last option: import=2, existing=13', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts[3]).toEqual({ offset: 3, importStation: 2, existingStation: 13 });
    });

    it('import station decreases, existing station increases', () => {
      const opts = computeMergeOptions(suggestion, 8);
      for (let i = 1; i < opts.length; i++) {
        expect(opts[i]!.importStation).toBeLessThan(opts[i - 1]!.importStation);
        expect(opts[i]!.existingStation).toBeGreaterThan(opts[i - 1]!.existingStation);
      }
    });
  });

  describe('Back + reverse (non-partial)', () => {
    const suggestion = {
      importStationIndex: 5,
      existingStationIndex: 10,
      matchedShots: 3,
      direction: 'reverse' as const,
      isPartial: false,
    };

    it('produces matchedShots+1 options', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts).toHaveLength(4);
    });

    it('first option: import=5, existing=10', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts[0]).toEqual({ offset: 0, importStation: 5, existingStation: 10 });
    });

    it('last option: import=2, existing=7', () => {
      const opts = computeMergeOptions(suggestion, 8);
      expect(opts[3]).toEqual({ offset: 3, importStation: 2, existingStation: 7 });
    });

    it('import station decreases, existing station decreases', () => {
      const opts = computeMergeOptions(suggestion, 8);
      for (let i = 1; i < opts.length; i++) {
        expect(opts[i]!.importStation).toBeLessThan(opts[i - 1]!.importStation);
        expect(opts[i]!.existingStation).toBeLessThan(opts[i - 1]!.existingStation);
      }
    });
  });

  it('partial match: maxOffset = matchedShots - 1', () => {
    const suggestion = {
      importStationIndex: 0,
      existingStationIndex: 10,
      matchedShots: 3,
      direction: 'forward' as const,
      isPartial: true,
    };
    const opts = computeMergeOptions(suggestion, 8);
    expect(opts).toHaveLength(3); // k=0,1,2 (not 3)
  });

  it('clamps import station to valid range', () => {
    // importStationIndex=1, matchedShots=3 → k can go up to 3
    // importStation = 1-k: 1, 0, -1 (clamped)
    const suggestion = {
      importStationIndex: 1,
      existingStationIndex: 5,
      matchedShots: 3,
      direction: 'forward' as const,
      isPartial: false,
    };
    const opts = computeMergeOptions(suggestion, 8);
    // k=0: import=1, k=1: import=0, k=2: import=-1 (skip), k=3: import=-2 (skip)
    expect(opts).toHaveLength(2);
    expect(opts[0]).toEqual({ offset: 0, importStation: 1, existingStation: 5 });
    expect(opts[1]).toEqual({ offset: 1, importStation: 0, existingStation: 6 });
  });

  it('clamps existing station to valid range (reverse)', () => {
    const suggestion = {
      importStationIndex: 0,
      existingStationIndex: 1,
      matchedShots: 3,
      direction: 'reverse' as const,
      isPartial: false,
    };
    const opts = computeMergeOptions(suggestion, 8);
    // k=0: existing=1, k=1: existing=0, k=2: existing=-1 (skip), k=3: existing=-2 (skip)
    expect(opts).toHaveLength(2);
    expect(opts[0]).toEqual({ offset: 0, importStation: 0, existingStation: 1 });
    expect(opts[1]).toEqual({ offset: 1, importStation: 1, existingStation: 0 });
  });

  it('ignores target-side extra-shot metadata for forward merge options', () => {
    const suggestion = {
      importStationIndex: 0,
      existingStationIndex: 18,
      matchedShots: 1,
      existingMatchedShots: 2,
      direction: 'forward' as const,
      isPartial: false,
    };

    const opts = computeMergeOptions(suggestion, 8);

    expect(opts).toEqual([
      { offset: 0, importStation: 0, existingStation: 18 },
      { offset: 1, importStation: 1, existingStation: 19 },
    ]);
  });

  it('ignores target-side extra-shot metadata for reverse merge options', () => {
    const suggestion = {
      importStationIndex: 0,
      existingStationIndex: 20,
      matchedShots: 1,
      existingMatchedShots: 2,
      direction: 'reverse' as const,
      isPartial: false,
    };

    const opts = computeMergeOptions(suggestion, 8);

    expect(opts).toEqual([
      { offset: 0, importStation: 0, existingStation: 20 },
      { offset: 1, importStation: 1, existingStation: 19 },
    ]);
  });
});

// ─── reverseTargetShot ──────────────────────────────────────────────────────

describe('reverseTargetShot', () => {
  const shot = makeTargetShot({
    azimuth_in: 45,
    azimuth_out: 46,
    length: 5.0,
    depth_in: 10,
    depth_out: 12,
    pitch_in: -5,
    pitch_out: -6,
    left: 2.0,
    right: 3.0,
    up: 1.5,
    down: 0.5,
  });

  it('adds 180° to azimuths and swaps in/out', () => {
    const reversed = reverseTargetShot(shot);
    expect(reversed.azimuth_in).toBe(226);  // (46+180) % 360
    expect(reversed.azimuth_out).toBe(225); // (45+180) % 360
  });

  it('keeps length unchanged', () => {
    const reversed = reverseTargetShot(shot);
    expect(reversed.length).toBe(5.0);
  });

  it('swaps depth_in and depth_out', () => {
    const reversed = reverseTargetShot(shot);
    expect(reversed.depth_in).toBe(12);
    expect(reversed.depth_out).toBe(10);
  });

  it('negates and swaps pitches', () => {
    const reversed = reverseTargetShot(shot);
    expect(reversed.pitch_in).toBe(6);   // -(-6)
    expect(reversed.pitch_out).toBe(5);  // -(-5)
  });

  it('swaps left and right', () => {
    const reversed = reverseTargetShot(shot);
    expect(reversed.left).toBe(3.0);
    expect(reversed.right).toBe(2.0);
  });

  it('keeps up and down unchanged', () => {
    const reversed = reverseTargetShot(shot);
    expect(reversed.up).toBe(1.5);
    expect(reversed.down).toBe(0.5);
  });

  it('handles null pitches', () => {
    const nullPitch = makeTargetShot({ pitch_in: null, pitch_out: null });
    const reversed = reverseTargetShot(nullPitch);
    expect(reversed.pitch_in).toBeNull();
    expect(reversed.pitch_out).toBeNull();
  });

  it('handles azimuth wrapping past 360°', () => {
    const highAz = makeTargetShot({ azimuth_in: 200, azimuth_out: 190 });
    const reversed = reverseTargetShot(highAz);
    expect(reversed.azimuth_in).toBe(10);  // (190+180) % 360
    expect(reversed.azimuth_out).toBe(20); // (200+180) % 360
  });
});

// ─── Agreement with the backend reverse ─────────────────────────────────────

describe('reverseTargetShot matches Project::reverse_section', () => {
  // The round-trip tests below cannot catch either of these: both bugs make the
  // transform an IDENTITY (or a sign-flip that undoes itself), and an identity
  // round-trips perfectly.

  it('swaps a degenerate antipodal pair instead of returning it unchanged', () => {
    // Readings exactly 180 apart have an undefined circular mean, so
    // reciprocate-and-swap maps the pair to itself and the shot keeps its old
    // bearing while everything around it turns around. `reverse_shots` swaps such
    // a pair; without that branch this returned {45, 225} - the input - i.e. it
    // disagreed with the backend by a full 180 degrees.
    const reversed = reverseTargetShot(makeTargetShot({ azimuth_in: 45, azimuth_out: 225 }));
    expect(reversed.azimuth_in).toBe(225);
    expect(reversed.azimuth_out).toBe(45);
  });

  it('keeps both legs in [0, 360) for an out-of-range reading', () => {
    // JS `%` is truncated, so its range is (-360, 360): `(-200 + 180) % 360` is
    // -20, a non-ignorable INVALID_AZIMUTH at whole-degree scale.
    expect((-200 + 180) % 360).toBe(-20);

    const reversed = reverseTargetShot(makeTargetShot({ azimuth_in: 20, azimuth_out: -200 }));
    for (const value of [reversed.azimuth_in, reversed.azimuth_out]) {
      expect(value).toBeGreaterThanOrEqual(0);
      expect(value).toBeLessThan(360);
    }
    // -200 normalises to 160, so the pair is (20, 160): not antipodal, ordinary
    // reciprocate-and-swap, exactly what the backend does after ITS pre-normalise.
    expect(reversed.azimuth_in).toBe(340);
    expect(reversed.azimuth_out).toBe(200);
  });
});

// ─── Round-trip reversal ────────────────────────────────────────────────────

describe('reverseTargetShot round-trip', () => {
  it('double reversal returns original values', () => {
    const original = makeTargetShot();
    const roundTrip = reverseTargetShot(reverseTargetShot(original));

    expect(roundTrip.azimuth_in).toBeCloseTo(original.azimuth_in, 10);
    expect(roundTrip.azimuth_out).toBeCloseTo(original.azimuth_out, 10);
    expect(roundTrip.length).toBe(original.length);
    expect(roundTrip.depth_in).toBe(original.depth_in);
    expect(roundTrip.depth_out).toBe(original.depth_out);
    expect(roundTrip.pitch_in).toBe(original.pitch_in);
    expect(roundTrip.pitch_out).toBe(original.pitch_out);
    expect(roundTrip.left).toBe(original.left);
    expect(roundTrip.right).toBe(original.right);
    expect(roundTrip.up).toBe(original.up);
    expect(roundTrip.down).toBe(original.down);
  });

  it('round-trip works with null pitches and null dimensions', () => {
    const original = makeTargetShot({
      pitch_in: null,
      pitch_out: null,
      left: null,
      right: null,
    });
    const roundTrip = reverseTargetShot(reverseTargetShot(original));

    expect(roundTrip.pitch_in).toBeNull();
    expect(roundTrip.pitch_out).toBeNull();
    expect(roundTrip.left).toBeNull();
    expect(roundTrip.right).toBeNull();
  });
});

// ─── applyTargetData ────────────────────────────────────────────────────────

describe('applyTargetData', () => {
  it('replaces measurement fields from target', () => {
    const shot = makeEditableShot(0);
    const target = makeTargetShot({ azimuth_in: 200, length: 10 });
    const result = applyTargetData(shot, target);

    expect(result.azimuth_in).toBe(200);
    expect(result.length).toBe(10);
    // Preserved from shot
    expect(result.id).toBe(0);
    expect(result.comment).toBeNull();
    expect(result.kind).toBe('Normal');
  });
});

// ─── interpolateShots ───────────────────────────────────────────────────────

describe('interpolateShots', () => {
  it('averages numeric fields', () => {
    const shot = makeEditableShot(0, { length: 4.0, depth_in: 8, depth_out: 10 });
    const target = makeTargetShot({ length: 6.0, depth_in: 12, depth_out: 14 });
    const result = interpolateShots(shot, target);

    expect(result.length).toBe(5.0);
    expect(result.depth_in).toBe(10);
    expect(result.depth_out).toBe(12);
  });

  it('uses circular mean for azimuths', () => {
    const shot = makeEditableShot(0, { azimuth_in: 350, azimuth_out: 355 });
    const target = makeTargetShot({ azimuth_in: 10, azimuth_out: 5 });
    const result = interpolateShots(shot, target);

    expect(result.azimuth_in).toBeCloseTo(0, 5);
    expect(result.azimuth_out).toBeCloseTo(0, 5);
  });

  it('preserves non-measurement fields', () => {
    const shot = makeEditableShot(42, { comment: 'test', kind: 'Reference' as const });
    const target = makeTargetShot();
    const result = interpolateShots(shot, target);

    expect(result.id).toBe(42);
    expect(result.comment).toBe('test');
    expect(result.kind).toBe('Reference');
  });
});

// ─── avgNullable ────────────────────────────────────────────────────────────

describe('avgNullable', () => {
  it('averages two numbers', () => {
    expect(avgNullable(4, 6)).toBe(5);
  });

  it('returns non-null when one is null', () => {
    expect(avgNullable(4, null)).toBe(4);
    expect(avgNullable(null, 6)).toBe(6);
  });

  it('returns null when both are null', () => {
    expect(avgNullable(null, null)).toBeNull();
  });
});

// ─── Strict reverse (←) direction regression tests ─────────────────────────
//
// The reverse direction was the source of the original bug: incorrect
// needsReversal logic, wrong target index mapping, and wrong merge option
// formulas.  These tests pin down exact values for every reverse scenario
// to prevent regressions.

describe('reverse direction: computeTargetShotIndex exact values', () => {
  describe('Front + reverse, existingStationIndex=10, matchedShots=3', () => {
    const params: OverlapParams = {
      importStationIndex: 0, matchedShots: 3,
      existingStationIndex: 10, direction: 'reverse',
    };
    // Formula: (existingStationIndex - 1) - k  where k = overlapId
    it.each([
      [0, 9],
      [1, 8],
      [2, 7],
    ])('overlapId=%i → target %i', (overlapId, expected) => {
      expect(computeTargetShotIndex(overlapId, params)).toBe(expected);
    });

    it('target indices are strictly decreasing as overlapId increases', () => {
      const indices = [0, 1, 2].map(id => computeTargetShotIndex(id, params));
      for (let i = 1; i < indices.length; i++) {
        expect(indices[i]!).toBeLessThan(indices[i - 1]!);
      }
    });
  });

  describe('Front + reverse, existingStationIndex=2, matchedShots=3 (near boundary)', () => {
    const params: OverlapParams = {
      importStationIndex: 0, matchedShots: 3,
      existingStationIndex: 2, direction: 'reverse',
    };
    it.each([
      [0, 1],   // k=0 → 2-1-0 = 1
      [1, 0],   // k=1 → 2-1-1 = 0
      [2, -1],  // k=2 → 2-1-2 = -1  (out of bounds - caller must check)
    ])('overlapId=%i → target %i', (overlapId, expected) => {
      expect(computeTargetShotIndex(overlapId, params)).toBe(expected);
    });
  });

  describe('Front + reverse, matchedShots=5 (larger overlap)', () => {
    const params: OverlapParams = {
      importStationIndex: 0, matchedShots: 5,
      existingStationIndex: 8, direction: 'reverse',
    };
    it.each([
      [0, 7],
      [1, 6],
      [2, 5],
      [3, 4],
      [4, 3],
    ])('overlapId=%i → target %i', (overlapId, expected) => {
      expect(computeTargetShotIndex(overlapId, params)).toBe(expected);
    });
  });

  describe('Back + reverse, importStationIndex=5, matchedShots=3', () => {
    const params: OverlapParams = {
      importStationIndex: 5, matchedShots: 3,
      existingStationIndex: 10, direction: 'reverse',
    };
    // Formula: (existingStationIndex - matchedShots) + k  where k = overlapId - overlapStart
    // overlapStart = 5 - 3 = 2
    it.each([
      [2, 7],   // k=0 → 10-3+0 = 7
      [3, 8],   // k=1 → 10-3+1 = 8
      [4, 9],   // k=2 → 10-3+2 = 9
    ])('overlapId=%i → target %i', (overlapId, expected) => {
      expect(computeTargetShotIndex(overlapId, params)).toBe(expected);
    });

    it('target indices are strictly increasing as overlapId increases', () => {
      const indices = [2, 3, 4].map(id => computeTargetShotIndex(id, params));
      for (let i = 1; i < indices.length; i++) {
        expect(indices[i]!).toBeGreaterThan(indices[i - 1]!);
      }
    });
  });

  describe('Back + reverse, importStationIndex=7, matchedShots=5 (larger)', () => {
    const params: OverlapParams = {
      importStationIndex: 7, matchedShots: 5,
      existingStationIndex: 12, direction: 'reverse',
    };
    // overlapStart = 7 - 5 = 2, formula = (12 - 5) + k = 7 + k
    it.each([
      [2, 7],
      [3, 8],
      [4, 9],
      [5, 10],
      [6, 11],
    ])('overlapId=%i → target %i', (overlapId, expected) => {
      expect(computeTargetShotIndex(overlapId, params)).toBe(expected);
    });
  });

  describe('Back + reverse, existingStationIndex=3, matchedShots=3 (near boundary)', () => {
    const params: OverlapParams = {
      importStationIndex: 4, matchedShots: 3,
      existingStationIndex: 3, direction: 'reverse',
    };
    // overlapStart = 4 - 3 = 1, formula = (3 - 3) + k = 0 + k
    it.each([
      [1, 0],
      [2, 1],
      [3, 2],
    ])('overlapId=%i → target %i', (overlapId, expected) => {
      expect(computeTargetShotIndex(overlapId, params)).toBe(expected);
    });
  });
});

describe('reverse direction: computeMergeOptions exact values', () => {
  describe('Front + reverse, all intermediate options', () => {
    const suggestion = {
      importStationIndex: 0, existingStationIndex: 10,
      matchedShots: 4, direction: 'reverse' as const, isPartial: false,
    };
    const opts = computeMergeOptions(suggestion, 10);

    it('produces 5 options (k=0..4)', () => {
      expect(opts).toHaveLength(5);
    });

    it.each([
      [0, 0, 10],
      [1, 1, 9],
      [2, 2, 8],
      [3, 3, 7],
      [4, 4, 6],
    ])('k=%i → import=%i, existing=%i', (offset, importStation, existingStation) => {
      expect(opts[offset]).toEqual({ offset, importStation, existingStation });
    });
  });

  describe('Back + reverse, all intermediate options', () => {
    const suggestion = {
      importStationIndex: 6, existingStationIndex: 10,
      matchedShots: 4, direction: 'reverse' as const, isPartial: false,
    };
    const opts = computeMergeOptions(suggestion, 10);

    it('produces 5 options (k=0..4)', () => {
      expect(opts).toHaveLength(5);
    });

    it.each([
      [0, 6, 10],
      [1, 5, 9],
      [2, 4, 8],
      [3, 3, 7],
      [4, 2, 6],
    ])('k=%i → import=%i, existing=%i', (offset, importStation, existingStation) => {
      expect(opts[offset]).toEqual({ offset, importStation, existingStation });
    });
  });

  describe('Front + reverse, partial match', () => {
    const suggestion = {
      importStationIndex: 0, existingStationIndex: 5,
      matchedShots: 3, direction: 'reverse' as const, isPartial: true,
    };
    const opts = computeMergeOptions(suggestion, 8);

    it('produces matchedShots options (not matchedShots+1)', () => {
      expect(opts).toHaveLength(3);
    });

    it.each([
      [0, 0, 5],
      [1, 1, 4],
      [2, 2, 3],
    ])('k=%i → import=%i, existing=%i', (offset, importStation, existingStation) => {
      expect(opts[offset]).toEqual({ offset, importStation, existingStation });
    });
  });

  describe('Back + reverse, existing near 0 (boundary clamping)', () => {
    const suggestion = {
      importStationIndex: 4, existingStationIndex: 2,
      matchedShots: 3, direction: 'reverse' as const, isPartial: false,
    };
    const opts = computeMergeOptions(suggestion, 8);

    // k=0: import=4, existing=2
    // k=1: import=3, existing=1
    // k=2: import=2, existing=0
    // k=3: import=1, existing=-1 → clamped
    it('clamps at existing=0', () => {
      expect(opts).toHaveLength(3);
      expect(opts[0]).toEqual({ offset: 0, importStation: 4, existingStation: 2 });
      expect(opts[1]).toEqual({ offset: 1, importStation: 3, existingStation: 1 });
      expect(opts[2]).toEqual({ offset: 2, importStation: 2, existingStation: 0 });
    });
  });
});

describe('reverse direction: needsDirectionReversal + reverseTargetShot pipeline', () => {
  // The core invariant: when needsReversal is true, applying reverseTargetShot
  // to a target heading ~X° must produce a heading ~(X+180)%360.
  // When needsReversal is false, target data passes through unchanged.

  it('Front + reverse: target ~270° becomes ~90° after reversal', () => {
    expect(needsDirectionReversal(0, 'reverse')).toBe(true);
    const target = makeTargetShot({ azimuth_in: 270, azimuth_out: 272 });
    const reversed = reverseTargetShot(target);
    expect(reversed.azimuth_in).toBe(92);   // (272+180) % 360
    expect(reversed.azimuth_out).toBe(90);  // (270+180) % 360
  });

  it('Front + reverse: target ~0° becomes ~180° after reversal', () => {
    expect(needsDirectionReversal(0, 'reverse')).toBe(true);
    const target = makeTargetShot({ azimuth_in: 2, azimuth_out: 358 });
    const reversed = reverseTargetShot(target);
    expect(reversed.azimuth_in).toBe(178);  // (358+180) % 360
    expect(reversed.azimuth_out).toBe(182); // (2+180) % 360
  });

  it('Back + reverse: NO reversal applied - target passes through', () => {
    expect(needsDirectionReversal(5, 'reverse')).toBe(false);
    const target = makeTargetShot({ azimuth_in: 90, azimuth_out: 91 });
    // No reverseTargetShot applied - values should be identical
    const result = applyTargetData(makeEditableShot(0), target);
    expect(result.azimuth_in).toBe(90);
    expect(result.azimuth_out).toBe(91);
  });

  it('Front + reverse: depths swap through reversal', () => {
    expect(needsDirectionReversal(0, 'reverse')).toBe(true);
    const target = makeTargetShot({ depth_in: 5, depth_out: 15 });
    const reversed = reverseTargetShot(target);
    expect(reversed.depth_in).toBe(15);
    expect(reversed.depth_out).toBe(5);
  });

  it('Front + reverse: pitches negate and swap through reversal', () => {
    expect(needsDirectionReversal(0, 'reverse')).toBe(true);
    const target = makeTargetShot({ pitch_in: -10, pitch_out: 3 });
    const reversed = reverseTargetShot(target);
    expect(reversed.pitch_in).toBe(-3);  // -(pitch_out)
    expect(reversed.pitch_out).toBe(10); // -(pitch_in)
  });

  it('Front + reverse: left/right swap through reversal', () => {
    expect(needsDirectionReversal(0, 'reverse')).toBe(true);
    const target = makeTargetShot({ left: 1.0, right: 4.0 });
    const reversed = reverseTargetShot(target);
    expect(reversed.left).toBe(4.0);
    expect(reversed.right).toBe(1.0);
  });
});

describe('reverse direction: computeActionableIds', () => {
  it('Front + reverse: actionable IDs work identically to Front + forward', () => {
    // computeActionableIds doesn't depend on direction - only on
    // importStationIndex and mergeImportStation. Verify this.
    const actionableFwd = computeActionableIds(0, 4, 2);
    // Should be same regardless of direction since direction is not a parameter
    expect(actionableFwd).toEqual(new Set([0, 1]));
  });

  it('Back + reverse: actionable IDs with various merge stations', () => {
    // importStationIndex=6, matchedShots=4, overlap={2,3,4,5}
    // mergeImportStation=4 → actionable = shots 4..5 = {4,5}
    expect(computeActionableIds(6, 4, 4)).toEqual(new Set([4, 5]));
    // mergeImportStation=2 → actionable = shots 2..5 = {2,3,4,5}
    expect(computeActionableIds(6, 4, 2)).toEqual(new Set([2, 3, 4, 5]));
    // mergeImportStation=5 → actionable = shots 5..5 = {5}
    expect(computeActionableIds(6, 4, 5)).toEqual(new Set([5]));
    // mergeImportStation=6 → actionable = shots 6..5 = {} (empty)
    expect(computeActionableIds(6, 4, 6)).toEqual(new Set());
  });
});

describe('reverse direction: full pipeline with concrete data', () => {
  /**
   * Concrete scenario for Front + reverse:
   *
   * Existing section "Main Passage" has 6 shots heading west (~270°):
   *   shot[0]: az=268/269  shot[1]: az=270/271  shot[2]: az=272/273
   *   shot[3]: az=269/270  shot[4]: az=271/272  shot[5]: az=273/274
   *
   * Import section "Side Passage" has 5 shots heading east (~90°).
   * It overlaps 3 shots at the front (importStationIndex=0), reverse direction.
   * existingStationIndex=4 means the existing anchor is station 4.
   *
   * Expected target mapping (Front + reverse):
   *   overlapId=0 → target=3  (k=0, existingStationIndex-1-0 = 3)
   *   overlapId=1 → target=2  (k=1, existingStationIndex-1-1 = 2)
   *   overlapId=2 → target=1  (k=2, existingStationIndex-1-2 = 1)
   *
   * Since needsReversal=true (Front+reverse), each target shot's ~270° heading
   * gets reversed to ~90° to match the import direction.
   */

  const existingShots: TargetShotLike[] = [
    makeTargetShot({ azimuth_in: 268, azimuth_out: 269, length: 3.0, depth_in: 0, depth_out: 2, pitch_in: -2, pitch_out: -3, left: 1.0, right: 2.0 }),
    makeTargetShot({ azimuth_in: 270, azimuth_out: 271, length: 3.5, depth_in: 2, depth_out: 4, pitch_in: -1, pitch_out: -2, left: 1.5, right: 2.5 }),
    makeTargetShot({ azimuth_in: 272, azimuth_out: 273, length: 4.0, depth_in: 4, depth_out: 6, pitch_in: 0,  pitch_out: -1, left: 2.0, right: 3.0 }),
    makeTargetShot({ azimuth_in: 269, azimuth_out: 270, length: 3.2, depth_in: 6, depth_out: 8, pitch_in: -3, pitch_out: -4, left: 1.8, right: 2.8 }),
    makeTargetShot({ azimuth_in: 271, azimuth_out: 272, length: 3.8, depth_in: 8, depth_out: 10, pitch_in: -2, pitch_out: -3, left: 2.2, right: 3.2 }),
    makeTargetShot({ azimuth_in: 273, azimuth_out: 274, length: 4.2, depth_in: 10, depth_out: 12, pitch_in: -1, pitch_out: -2, left: 2.5, right: 3.5 }),
  ];

  const importShots: EditableShotLike[] = Array.from({ length: 5 }, (_, i) =>
    makeEditableShot(i, {
      azimuth_in: 88 + i, azimuth_out: 89 + i,
      length: 3.0 + i * 0.2,
      depth_in: i * 2, depth_out: (i + 1) * 2,
    }),
  );

  const params: OverlapParams = {
    importStationIndex: 0, matchedShots: 3,
    existingStationIndex: 4, direction: 'reverse',
  };

  it('maps overlap IDs to correct target indices', () => {
    expect(computeTargetShotIndex(0, params)).toBe(3);
    expect(computeTargetShotIndex(1, params)).toBe(2);
    expect(computeTargetShotIndex(2, params)).toBe(1);
  });

  it('existing_data mode: each overlap shot gets exact reversed target values', () => {
    const shouldReverse = needsDirectionReversal(0, 'reverse');
    expect(shouldReverse).toBe(true);

    // overlapId=0 → target shot[3]
    const t3 = existingShots[3]!;
    const r3 = reverseTargetShot(t3);
    const applied0 = applyTargetData(importShots[0]!, r3);
    expect(applied0.azimuth_in).toBe((t3.azimuth_out + 180) % 360);  // (270+180)%360 = 90
    expect(applied0.azimuth_out).toBe((t3.azimuth_in + 180) % 360);  // (269+180)%360 = 89
    expect(applied0.length).toBe(t3.length);
    expect(applied0.depth_in).toBe(t3.depth_out);   // swapped: 8
    expect(applied0.depth_out).toBe(t3.depth_in);   // swapped: 6
    expect(applied0.pitch_in).toBe(4);               // -(pitch_out of t3) = -(-4)
    expect(applied0.pitch_out).toBe(3);              // -(pitch_in of t3)  = -(-3)
    expect(applied0.left).toBe(t3.right);            // swapped: 2.8
    expect(applied0.right).toBe(t3.left);            // swapped: 1.8

    // overlapId=1 → target shot[2]
    const t2 = existingShots[2]!;
    const r2 = reverseTargetShot(t2);
    const applied1 = applyTargetData(importShots[1]!, r2);
    expect(applied1.azimuth_in).toBe((t2.azimuth_out + 180) % 360);  // (273+180)%360 = 93
    expect(applied1.azimuth_out).toBe((t2.azimuth_in + 180) % 360);  // (272+180)%360 = 92
    expect(applied1.depth_in).toBe(t2.depth_out);   // 6
    expect(applied1.depth_out).toBe(t2.depth_in);   // 4
    expect(applied1.pitch_in).toBe(1);               // -(pitch_out) = -(-1)
    expect(applied1.pitch_out).toBe(-0);             // -(pitch_in)  = -(0) = -0
    expect(applied1.left).toBe(t2.right);            // 3.0
    expect(applied1.right).toBe(t2.left);            // 2.0

    // overlapId=2 → target shot[1]
    const t1 = existingShots[1]!;
    const r1 = reverseTargetShot(t1);
    const applied2 = applyTargetData(importShots[2]!, r1);
    expect(applied2.azimuth_in).toBe((t1.azimuth_out + 180) % 360);  // (271+180)%360 = 91
    expect(applied2.azimuth_out).toBe((t1.azimuth_in + 180) % 360);  // (270+180)%360 = 90
    expect(applied2.depth_in).toBe(t1.depth_out);
    expect(applied2.depth_out).toBe(t1.depth_in);
  });

  it('interpolate mode: averages import + reversed target', () => {
    const shouldReverse = needsDirectionReversal(0, 'reverse');
    expect(shouldReverse).toBe(true);

    // overlapId=0: import shot heading ~88°, reversed target heading ~90°
    const t3 = existingShots[3]!;
    const r3 = reverseTargetShot(t3);
    const interp = interpolateShots(importShots[0]!, r3);

    // azimuth: circular mean of 88 and 90 ≈ 89
    expect(interp.azimuth_in).toBeCloseTo(89, 0);
    // length: (3.0 + 3.2) / 2 = 3.1
    expect(interp.length).toBeCloseTo(3.1, 5);
    // depth_in: (0 + 8) / 2 = 4   (import=0, reversed target=depth_out of t3=8)
    expect(interp.depth_in).toBe(4);
    // id preserved from import
    expect(interp.id).toBe(0);
  });

  it('non-overlap shots are unaffected', () => {
    const overlapIds = computeOverlapIds(0, 3);
    // Shot IDs 3, 4 are not in overlap
    expect(overlapIds.has(3)).toBe(false);
    expect(overlapIds.has(4)).toBe(false);
  });
});

describe('reverse direction: full pipeline Back + reverse', () => {
  /**
   * Back + reverse scenario:
   *
   * Import section has 7 shots, overlap at back (importStationIndex=7).
   * 3 matched shots: overlap IDs = {4, 5, 6}.
   * existingStationIndex=8, direction=reverse.
   *
   * needsReversal = false (Back + reverse).
   * Target index = (existingStationIndex - matchedShots) + k = (8 - 3) + k = 5 + k
   *   overlapId=4 (k=0) → target=5
   *   overlapId=5 (k=1) → target=6
   *   overlapId=6 (k=2) → target=7
   *
   * Since no reversal: target data is used as-is.
   */

  const existingShots: TargetShotLike[] = Array.from({ length: 10 }, (_, i) =>
    makeTargetShot({
      azimuth_in: 90 + i, azimuth_out: 91 + i,
      length: 2.5 + i * 0.1,
      depth_in: i * 3, depth_out: (i + 1) * 3,
      pitch_in: -(i + 1), pitch_out: -(i + 2),
      left: 1.0 + i * 0.2, right: 2.0 + i * 0.2,
    }),
  );

  const params: OverlapParams = {
    importStationIndex: 7, matchedShots: 3,
    existingStationIndex: 8, direction: 'reverse',
  };

  it('maps overlap IDs to correct target indices', () => {
    expect(computeTargetShotIndex(4, params)).toBe(5);
    expect(computeTargetShotIndex(5, params)).toBe(6);
    expect(computeTargetShotIndex(6, params)).toBe(7);
  });

  it('needsReversal is false - target data passes through unchanged', () => {
    expect(needsDirectionReversal(7, 'reverse')).toBe(false);

    const importShot = makeEditableShot(4);
    const target = existingShots[5]!; // target for overlapId=4
    const applied = applyTargetData(importShot, target);

    // All values come directly from target, no reversal
    expect(applied.azimuth_in).toBe(95);    // 90 + 5
    expect(applied.azimuth_out).toBe(96);   // 91 + 5
    expect(applied.length).toBe(3.0);       // 2.5 + 5*0.1
    expect(applied.depth_in).toBe(15);      // 5 * 3
    expect(applied.depth_out).toBe(18);     // 6 * 3
    expect(applied.pitch_in).toBe(-6);      // -(5+1)
    expect(applied.pitch_out).toBe(-7);     // -(5+2)
    expect(applied.left).toBe(2.0);         // 1.0 + 5*0.2
    expect(applied.right).toBe(3.0);        // 2.0 + 5*0.2
  });

  it('merge options all have decreasing existing stations', () => {
    const suggestion = {
      importStationIndex: 7, existingStationIndex: 8,
      matchedShots: 3, direction: 'reverse' as const, isPartial: false,
    };
    const opts = computeMergeOptions(suggestion, 10);
    // k=0: import=7, existing=8
    // k=1: import=6, existing=7
    // k=2: import=5, existing=6
    // k=3: import=4, existing=5
    expect(opts).toHaveLength(4);
    expect(opts[0]).toEqual({ offset: 0, importStation: 7, existingStation: 8 });
    expect(opts[3]).toEqual({ offset: 3, importStation: 4, existingStation: 5 });
    for (let i = 1; i < opts.length; i++) {
      expect(opts[i]!.existingStation).toBeLessThan(opts[i - 1]!.existingStation);
    }
  });
});

// ─── Integration: overlap mode does not flip azimuths 180° ─────────────────

describe('integration: overlap mode correctness', () => {
  /**
   * Scenario: A 5-shot import section overlapping 3 shots with an existing
   * section. The import is heading ~90° and the target is also ~90°.
   *
   * After applying overlap data (existing_data mode), the resulting azimuths
   * should still be ~90°, NOT ~270° (which would be the original bug from
   * incorrect reversal logic).
   */

  // Existing section shots heading ~90° (east)
  const targetShots: TargetShotLike[] = Array.from({ length: 8 }, (_, i) =>
    makeTargetShot({
      azimuth_in: 88 + i,
      azimuth_out: 89 + i,
      length: 3.0 + i * 0.1,
      depth_in: i * 2,
      depth_out: (i + 1) * 2,
    }),
  );

  // Import shots heading ~90° (same direction)
  const importShots: EditableShotLike[] = Array.from({ length: 5 }, (_, i) =>
    makeEditableShot(i, {
      azimuth_in: 89 + i,
      azimuth_out: 90 + i,
      length: 3.1 + i * 0.1,
      depth_in: i * 2,
      depth_out: (i + 1) * 2,
    }),
  );

  it('Front + forward: existing_data preserves ~90° heading', () => {
    const params: OverlapParams = {
      importStationIndex: 0,
      matchedShots: 3,
      existingStationIndex: 5,
      direction: 'forward',
    };
    const reversed = needsDirectionReversal(0, 'forward');
    expect(reversed).toBe(false);

    const overlapIds = computeOverlapIds(0, 3);

    for (const overlapId of overlapIds) {
      const targetIdx = computeTargetShotIndex(overlapId, params);
      const target = targetShots[targetIdx]!;
      const applied = applyTargetData(importShots[overlapId]!, reversed ? reverseTargetShot(target) : target);
      // Azimuth should still be ~90°, not ~270°
      expect(applied.azimuth_in).toBeGreaterThan(80);
      expect(applied.azimuth_in).toBeLessThan(100);
    }
  });

  it('Front + reverse: existing_data preserves ~90° heading after reversal', () => {
    // Target is heading ~270° (opposite), import is heading ~90°.
    // Reversal flips target back to ~90°.
    const reverseTargets: TargetShotLike[] = Array.from({ length: 8 }, (_, i) =>
      makeTargetShot({
        azimuth_in: 268 + i,
        azimuth_out: 269 + i,
        length: 3.0 + i * 0.1,
        depth_in: i * 2,
        depth_out: (i + 1) * 2,
        pitch_in: 5,
        pitch_out: 6,
      }),
    );

    const params: OverlapParams = {
      importStationIndex: 0,
      matchedShots: 3,
      existingStationIndex: 5,
      direction: 'reverse',
    };
    const reversed = needsDirectionReversal(0, 'reverse');
    expect(reversed).toBe(true);

    const overlapIds = computeOverlapIds(0, 3);

    for (const overlapId of overlapIds) {
      const targetIdx = computeTargetShotIndex(overlapId, params);
      const target = reverseTargets[targetIdx]!;
      const applied = applyTargetData(importShots[overlapId]!, reversed ? reverseTargetShot(target) : target);
      // After reversal of ~270° target, azimuth should be ~90°
      expect(applied.azimuth_in).toBeGreaterThan(80);
      expect(applied.azimuth_in).toBeLessThan(100);
    }
  });

  it('Back + forward: existing_data preserves ~90° heading after reversal', () => {
    // Back+forward needs reversal. Target is heading ~270°.
    const reverseTargets: TargetShotLike[] = Array.from({ length: 15 }, (_, i) =>
      makeTargetShot({
        azimuth_in: 268 + (i % 10),
        azimuth_out: 269 + (i % 10),
        length: 3.0 + i * 0.1,
        depth_in: i * 2,
        depth_out: (i + 1) * 2,
        pitch_in: 5,
        pitch_out: 6,
      }),
    );

    const params: OverlapParams = {
      importStationIndex: 5,
      matchedShots: 3,
      existingStationIndex: 10,
      direction: 'forward',
    };
    const reversed = needsDirectionReversal(5, 'forward');
    expect(reversed).toBe(true);

    const overlapIds = computeOverlapIds(5, 3);

    for (const overlapId of overlapIds) {
      const targetIdx = computeTargetShotIndex(overlapId, params);
      const target = reverseTargets[targetIdx]!;
      const applied = applyTargetData(importShots[overlapId]!, reversed ? reverseTargetShot(target) : target);
      expect(applied.azimuth_in).toBeGreaterThan(80);
      expect(applied.azimuth_in).toBeLessThan(100);
    }
  });

  it('Back + reverse: existing_data preserves ~90° heading (no reversal)', () => {
    const params: OverlapParams = {
      importStationIndex: 5,
      matchedShots: 3,
      existingStationIndex: 10,
      direction: 'reverse',
    };
    const reversed = needsDirectionReversal(5, 'reverse');
    expect(reversed).toBe(false);

    const overlapIds = computeOverlapIds(5, 3);

    // Target also heading ~90° (same direction, no reversal needed)
    const sameDirectionTargets: TargetShotLike[] = Array.from({ length: 12 }, (_, i) =>
      makeTargetShot({
        azimuth_in: 88 + (i % 10),
        azimuth_out: 89 + (i % 10),
        length: 3.0 + i * 0.1,
        depth_in: i * 2,
        depth_out: (i + 1) * 2,
      }),
    );

    for (const overlapId of overlapIds) {
      const targetIdx = computeTargetShotIndex(overlapId, params);
      const target = sameDirectionTargets[targetIdx]!;
      const applied = applyTargetData(importShots[overlapId]!, reversed ? reverseTargetShot(target) : target);
      expect(applied.azimuth_in).toBeGreaterThan(80);
      expect(applied.azimuth_in).toBeLessThan(100);
    }
  });
});

// ─── Geometric invariant: station coordinates stay close across overlap modes ─

/**
 * Walk a sequence of shots and return station (x, y, z) coordinates.
 * Uses the same formula as SectionPreview:
 *   azimuth = average(azimuth_in, azimuth_out) in degrees
 *   x += length * sin(azimuthRad)    (east)
 *   y += length * cos(azimuthRad)    (north)
 *   z += depth_out - depth_in        (depth)
 */
function walkStations(shots: TargetShotLike[]): Array<{ x: number; y: number; z: number }> {
  const stations = [{ x: 0, y: 0, z: 0 }];
  for (const s of shots) {
    const prev = stations[stations.length - 1]!;
    // Simple average - fine when azimuth_in ≈ azimuth_out (no wrap-around issues)
    const az = ((s.azimuth_in + s.azimuth_out) / 2) * (Math.PI / 180);
    stations.push({
      x: prev.x + s.length * Math.sin(az),
      y: prev.y + s.length * Math.cos(az),
      z: prev.z + (s.depth_out - s.depth_in),
    });
  }
  return stations;
}

function dist3d(
  a: { x: number; y: number; z: number },
  b: { x: number; y: number; z: number },
): number {
  return Math.sqrt((a.x - b.x) ** 2 + (a.y - b.y) ** 2 + (a.z - b.z) ** 2);
}

describe('geometric invariant: station coordinates stay close across overlap modes', () => {
  /**
   * Setup: two surveyors measured approximately the same passage.
   *
   * "existing" (target) section: 8 shots heading ~45° NE, gently descending.
   * "import" section: 8 shots heading ~45° NE with small random-ish offsets
   * (±0.8° azimuth, ±0.15m length, ±0.2m depth) to simulate real survey error.
   *
   * For each overlap case (Front/Back × Forward/Reverse):
   * - Walk the import overlap shots → importCoords
   * - Walk the target overlap shots (in import direction) → targetCoords
   * - Apply each overlap mode (existing_data, interpolate, import_data)
   * - Walk the result → resultCoords
   * - Assert resultCoords ≈ importCoords and ≈ targetCoords within tolerance
   *
   * The tolerance is generous (2m per station) because the point is NOT that
   * coordinates are identical, but that they don't diverge wildly (which would
   * happen if azimuths were flipped 180°, making a ~5m shot produce ~10m error).
   */

  // "Same-direction" existing section: 8 shots heading ~45° NE (same as import).
  // Used for Forward cases where needsReversal=false.
  const existingSameDir: TargetShotLike[] = Array.from({ length: 8 }, (_, i) => ({
    azimuth_in:  44 + i * 0.5,
    azimuth_out: 46 + i * 0.5,
    length: 5.0 + i * 0.05,
    depth_in:  i * 1.0,
    depth_out: (i + 1) * 1.0,
    pitch_in: -3, pitch_out: -3,
    left: 2.0, right: 2.0, up: 1.5, down: 0.5,
  }));

  // "Opposite-direction" existing section: heading ~225° SW (opposite of import).
  // Used for cases where needsReversal=true - reverseTargetShot flips ~225° → ~45°.
  // Created by reversing the same-direction shots, so the round-trip is exact.
  const existingOppositeDir: TargetShotLike[] = existingSameDir.map(reverseTargetShot);

  // Import section: same passage as existing, heading ~45° with small survey error
  const importShots: EditableShotLike[] = Array.from({ length: 8 }, (_, i) => ({
    id: i,
    azimuth_in:  45 + i * 0.5 + (i % 3 - 1) * 0.8,   // ±0.8° jitter
    azimuth_out: 47 + i * 0.5 + (i % 3 - 1) * 0.8,
    length: 5.1 + i * 0.05 + (i % 2 === 0 ? 0.15 : -0.15),  // ±0.15m
    depth_in:  i * 1.0 + (i % 2) * 0.2,
    depth_out: (i + 1) * 1.0 + (i % 2) * 0.2,
    pitch_in: -3, pitch_out: -3,
    left: 2.0, right: 2.0, up: 1.5, down: 0.5,
    comment: null, kind: 'Normal' as const,
  }));

  // Tolerance: generous enough for survey error, but catches any 180° flip.
  // A 180° flip on a 5m shot produces ~10m displacement - easily beyond 2m.
  const TOLERANCE_PER_STATION = 2.0;

  type Case = {
    label: string;
    importStationIndex: number;
    existingStationIndex: number;
    matchedShots: number;
    direction: 'forward' | 'reverse';
    // needsReversal: true → use oppositeDir existing shots (reversal flips them back)
    //                false → use sameDir existing shots (no reversal needed)
    existingShots: TargetShotLike[];
  };

  const cases: Case[] = [
    // Front+forward: needsReversal=false → existing heads same way (~45°)
    { label: 'Front + forward', importStationIndex: 0, existingStationIndex: 3, matchedShots: 3, direction: 'forward', existingShots: existingSameDir },
    // Front+reverse: needsReversal=true → existing heads opposite (~225°), reversal flips to ~45°
    { label: 'Front + reverse', importStationIndex: 0, existingStationIndex: 5, matchedShots: 3, direction: 'reverse', existingShots: existingOppositeDir },
    // Back+forward: needsReversal=true → existing heads opposite (~225°), reversal flips to ~45°
    { label: 'Back + forward',  importStationIndex: 8, existingStationIndex: 3, matchedShots: 3, direction: 'forward', existingShots: existingOppositeDir },
    // Back+reverse: needsReversal=false → existing heads same way (~45°)
    { label: 'Back + reverse',  importStationIndex: 8, existingStationIndex: 5, matchedShots: 3, direction: 'reverse', existingShots: existingSameDir },
  ];

  for (const c of cases) {
    describe(c.label, () => {
      const params: OverlapParams = {
        importStationIndex: c.importStationIndex,
        matchedShots: c.matchedShots,
        existingStationIndex: c.existingStationIndex,
        direction: c.direction,
      };
      const shouldReverse = needsDirectionReversal(c.importStationIndex, c.direction);
      const overlapIds = computeOverlapIds(c.importStationIndex, c.matchedShots);
      const sortedOverlapIds = [...overlapIds].sort((a, b) => a - b);

      // Extract the overlap shots from import and from target (in import direction)
      const importOverlapShots = sortedOverlapIds.map(id => importShots[id]!);
      const targetOverlapInImportDir = sortedOverlapIds.map(id => {
        const targetIdx = computeTargetShotIndex(id, params);
        const raw = c.existingShots[targetIdx]!;
        return shouldReverse ? reverseTargetShot(raw) : raw;
      });

      // Walk each to get reference coordinates
      const importCoords = walkStations(importOverlapShots);
      const targetCoords = walkStations(targetOverlapInImportDir);

      it('import and target overlap coordinates are close (sanity check)', () => {
        // If the data is set up correctly, import and target coords should be
        // within a few meters of each other (same passage, small error).
        for (let i = 0; i < importCoords.length; i++) {
          const d = dist3d(importCoords[i]!, targetCoords[i]!);
          expect(d).toBeLessThan(TOLERANCE_PER_STATION * (i + 1));
        }
      });

      it('existing_data mode: station coords ≈ target coords', () => {
        const resultShots = sortedOverlapIds.map(id => {
          const targetIdx = computeTargetShotIndex(id, params);
          const raw = c.existingShots[targetIdx]!;
          const target = shouldReverse ? reverseTargetShot(raw) : raw;
          return applyTargetData(importShots[id]!, target);
        });
        const resultCoords = walkStations(resultShots);

        for (let i = 0; i < resultCoords.length; i++) {
          // Should be essentially identical to target coords
          const d = dist3d(resultCoords[i]!, targetCoords[i]!);
          expect(d).toBeCloseTo(0, 5);
        }
      });

      it('existing_data mode: station coords ≈ import coords', () => {
        const resultShots = sortedOverlapIds.map(id => {
          const targetIdx = computeTargetShotIndex(id, params);
          const raw = c.existingShots[targetIdx]!;
          const target = shouldReverse ? reverseTargetShot(raw) : raw;
          return applyTargetData(importShots[id]!, target);
        });
        const resultCoords = walkStations(resultShots);

        for (let i = 0; i < resultCoords.length; i++) {
          const d = dist3d(resultCoords[i]!, importCoords[i]!);
          expect(d).toBeLessThan(TOLERANCE_PER_STATION * (i + 1));
        }
      });

      it('interpolate mode: station coords between import and target', () => {
        const resultShots = sortedOverlapIds.map(id => {
          const targetIdx = computeTargetShotIndex(id, params);
          const raw = c.existingShots[targetIdx]!;
          const target = shouldReverse ? reverseTargetShot(raw) : raw;
          return interpolateShots(importShots[id]!, target);
        });
        const resultCoords = walkStations(resultShots);

        for (let i = 1; i < resultCoords.length; i++) {
          const dImport = dist3d(resultCoords[i]!, importCoords[i]!);
          const dTarget = dist3d(resultCoords[i]!, targetCoords[i]!);
          // Interpolated should be close to both
          expect(dImport).toBeLessThan(TOLERANCE_PER_STATION * i);
          expect(dTarget).toBeLessThan(TOLERANCE_PER_STATION * i);
        }
      });

      it('import_data mode: station coords = import coords exactly', () => {
        // import_data keeps import shots as-is - coordinates must be identical
        const resultCoords = walkStations(importOverlapShots);
        for (let i = 0; i < resultCoords.length; i++) {
          expect(resultCoords[i]!.x).toBeCloseTo(importCoords[i]!.x, 10);
          expect(resultCoords[i]!.y).toBeCloseTo(importCoords[i]!.y, 10);
          expect(resultCoords[i]!.z).toBeCloseTo(importCoords[i]!.z, 10);
        }
      });

      it('NO 180° flip: final station within tolerance (not across the cave)', () => {
        // The most important check: if azimuth were flipped 180°, the final
        // station would be ~2*distance away from where it should be.
        // With matchedShots=3 at ~5m each, a flip would put the final station
        // ~30m away instead of ~15m from origin.
        const resultShots = sortedOverlapIds.map(id => {
          const targetIdx = computeTargetShotIndex(id, params);
          const raw = c.existingShots[targetIdx]!;
          const target = shouldReverse ? reverseTargetShot(raw) : raw;
          return applyTargetData(importShots[id]!, target);
        });
        const resultCoords = walkStations(resultShots);
        const last = resultCoords[resultCoords.length - 1]!;
        const importLast = importCoords[importCoords.length - 1]!;

        // With ~5m shots and ~1° error, max cumulative drift over 3 shots is < 1m.
        // A 180° flip would produce ~30m displacement.
        expect(dist3d(last, importLast)).toBeLessThan(5.0);
      });
    });
  }
});

// ─── Multi-connection overlap: target shot un-delete and update ──────────────
//
// When section A is the import for connection A→B (overlap shots in A deleted),
// and later section C connects to A (C→A), the fix must update A's target shots.
// If A's target shots were deleted by a DIFFERENT prior connection (e.g., A was
// the import to D), those shots must be un-deleted so they can serve as the
// surviving representative for the C→A overlap.

describe('multi-connection overlap: target shot updates', () => {
  // Section A has 10 shots. It was the import in A→D connection,
  // which deleted overlap shots 8,9 (Back overlap at station 10).
  // Now C→A connects at A's station 2 (Front overlap, 2 matched shots).
  // C→A should update A's shots at indices 2,3.
  const sectionA: EditableShotLike[] = Array.from({ length: 10 }, (_, i) =>
    makeEditableShot(i, { azimuth_in: 100 + i, length: 5 + i * 0.1, depth_in: i, depth_out: i + 1 }),
  );
  // Mark shots 8,9 as deleted (from A→D connection)
  sectionA[8] = { ...sectionA[8]!, isDeleted: true };
  sectionA[9] = { ...sectionA[9]!, isDeleted: true };

  const sectionC: EditableShotLike[] = [
    makeEditableShot(0, { azimuth_in: 200, length: 7.0, depth_in: 2, depth_out: 3 }),
    makeEditableShot(1, { azimuth_in: 210, length: 8.0, depth_in: 3, depth_out: 4 }),
    makeEditableShot(2, { azimuth_in: 220, length: 9.0, depth_in: 4, depth_out: 5 }),
  ];

  // C→A suggestion: C import_stn=0 (Front), A existing_stn=2, matched=2, forward
  const cToAParams: OverlapParams = {
    importStationIndex: 0,
    matchedShots: 2,
    existingStationIndex: 2,
    direction: 'forward',
  };

  it('updates non-deleted target shots with import_data', () => {
    const allOverlap = computeOverlapIds(0, 2); // {0, 1}
    const shouldReverse = needsDirectionReversal(0, 'forward');
    expect(shouldReverse).toBe(false);

    const modifiedA = sectionA.map(s => ({ ...s }));
    let modified = false;

    for (const overlapId of allOverlap) {
      const importShot = sectionC.find(s => s.id === overlapId);
      if (!importShot) continue;
      const targetIdx = computeTargetShotIndex(overlapId, cToAParams);
      if (targetIdx < 0 || targetIdx >= modifiedA.length) continue;
      const targetShot = modifiedA[targetIdx]!;
      if (targetShot.isDeleted) continue; // old behavior: skip deleted
      modifiedA[targetIdx] = applyTargetData(targetShot, importShot);
      modified = true;
    }

    expect(modified).toBe(true);
    // A[2] updated with C[0] values
    expect(modifiedA[2]!.azimuth_in).toBe(200);
    expect(modifiedA[2]!.length).toBe(7.0);
    // A[3] updated with C[1] values
    expect(modifiedA[3]!.azimuth_in).toBe(210);
    expect(modifiedA[3]!.length).toBe(8.0);
    // A[8] still deleted (from A→D, not in C→A overlap)
    expect(modifiedA[8]!.isDeleted).toBe(true);
  });

  it('un-deletes and updates target shots deleted by a previous connection', () => {
    // Now simulate a scenario where A's shots 2,3 were ALSO deleted
    // by a previous connection (e.g., A was import in A→E which overlapped at front)
    const sectionAWithFrontDeleted = sectionA.map(s => ({ ...s }));
    sectionAWithFrontDeleted[2] = { ...sectionAWithFrontDeleted[2]!, isDeleted: true };
    sectionAWithFrontDeleted[3] = { ...sectionAWithFrontDeleted[3]!, isDeleted: true };

    const allOverlap = computeOverlapIds(0, 2);
    const shouldReverse = needsDirectionReversal(0, 'forward');

    const modifiedA = sectionAWithFrontDeleted.map(s => ({ ...s }));
    let modified = false;

    for (const overlapId of allOverlap) {
      const importShot = sectionC.find(s => s.id === overlapId);
      if (!importShot) continue;
      const targetIdx = computeTargetShotIndex(overlapId, cToAParams);
      if (targetIdx < 0 || targetIdx >= modifiedA.length) continue;
      let targetShot = modifiedA[targetIdx]!;
      // NEW behavior: un-delete and apply
      if (targetShot.isDeleted) {
        targetShot = { ...targetShot, isDeleted: false };
        modifiedA[targetIdx] = targetShot;
      }
      const importAsTarget: TargetShotLike = shouldReverse
        ? reverseTargetShot(importShot)
        : importShot;
      modifiedA[targetIdx] = applyTargetData(targetShot, importAsTarget);
      modified = true;
    }

    expect(modified).toBe(true);
    // A[2] un-deleted and updated with C[0] values
    expect(modifiedA[2]!.isDeleted).toBeFalsy();
    expect(modifiedA[2]!.azimuth_in).toBe(200);
    expect(modifiedA[2]!.length).toBe(7.0);
    // A[3] un-deleted and updated with C[1] values
    expect(modifiedA[3]!.isDeleted).toBeFalsy();
    expect(modifiedA[3]!.azimuth_in).toBe(210);
    expect(modifiedA[3]!.length).toBe(8.0);
    // A[8] still deleted (not in the C→A overlap region)
    expect(modifiedA[8]!.isDeleted).toBe(true);
  });

  it('adjacent non-overlapping connections update different target shots', () => {
    // Two adjacent connections into one section:
    // Connection 1: B→A at A station 25, 2 matched, forward (updates A[25],A[26])
    // Connection 2: C→A at A station 27, 2 matched, forward (updates A[27],A[28])
    // These should be independent - no conflict.

    // Expand section A to 30 shots for this test
    const bigA: EditableShotLike[] = Array.from({ length: 30 }, (_, i) =>
      makeEditableShot(i, { azimuth_in: 100 + i, length: 5 + i * 0.1, depth_in: i, depth_out: i + 1 }),
    );

    const sectionB: EditableShotLike[] = [
      makeEditableShot(0, { azimuth_in: 300, length: 3.0, depth_in: 25, depth_out: 26 }),
      makeEditableShot(1, { azimuth_in: 310, length: 4.0, depth_in: 26, depth_out: 27 }),
    ];

    // B→A: B import_stn=2 (Back, 2 shots), A existing_stn=25, matched=2, forward
    const bToAParams: OverlapParams = {
      importStationIndex: 2,
      matchedShots: 2,
      existingStationIndex: 25,
      direction: 'forward',
    };

    // Apply B→A connection
    const modifiedA = bigA.map(s => ({ ...s }));
    const bOverlap = computeOverlapIds(2, 2); // {0, 1}
    for (const overlapId of bOverlap) {
      const importShot = sectionB.find(s => s.id === overlapId);
      if (!importShot) continue;
      const targetIdx = computeTargetShotIndex(overlapId, bToAParams);
      if (targetIdx < 0 || targetIdx >= modifiedA.length) continue;
      modifiedA[targetIdx] = applyTargetData(modifiedA[targetIdx]!, importShot);
    }

    // Verify B→A updated A[25] and A[26]
    // Back + forward: (25 + 2 - 1) - k
    // k=0 (overlapId=0): targetIdx = 26
    // k=1 (overlapId=1): targetIdx = 25
    expect(modifiedA[26]!.azimuth_in).toBe(300); // B[0] → A[26]
    expect(modifiedA[25]!.azimuth_in).toBe(310); // B[1] → A[25]

    // Apply C→A connection (C from the outer scope, existing_stn=27)
    const cOverlap = computeOverlapIds(0, 2); // {0, 1}
    for (const overlapId of cOverlap) {
      const importShot = sectionC.find(s => s.id === overlapId);
      if (!importShot) continue;
      const targetIdx = computeTargetShotIndex(overlapId, { ...cToAParams, existingStationIndex: 27 });
      if (targetIdx < 0 || targetIdx >= modifiedA.length) continue;
      modifiedA[targetIdx] = applyTargetData(modifiedA[targetIdx]!, importShot);
    }

    // Verify C→A updated A[27] and A[28] independently
    expect(modifiedA[27]!.azimuth_in).toBe(200); // C[0] → A[27]
    expect(modifiedA[28]!.azimuth_in).toBe(210); // C[1] → A[28]

    // B→A updates at A[25],A[26] should still be intact
    expect(modifiedA[26]!.azimuth_in).toBe(300);
    expect(modifiedA[25]!.azimuth_in).toBe(310);

    // Non-overlap shots should be unchanged
    expect(modifiedA[24]!.azimuth_in).toBe(124);
    expect(modifiedA[29]!.azimuth_in).toBe(129);
  });
});

// ─── handleConfirm target-shot-update simulation ────────────────────────────
//
// These tests simulate the exact code path in DmpNewImportDialog.handleConfirm
// that propagates overlap mode to the target section's shots for import-to-import
// connections. This is the fix for "interpolation not applied" bug.

describe('handleConfirm target-shot-update: a front overlap into an existing section', () => {
  /**
   * Invented suggestion parameters, with no survey file behind them:
   *   Import section: 9 shots, importStationIndex=0, matchedShots=3
   *   Target section: 15 shots, existingStationIndex=7, direction=forward
   *   Default merge station = importStationIndex = 0
   *
   * This is a FRONT overlap (importStationIndex=0).
   *
   * The bug: computeActionableIds(0, 3, 0) returns an empty set because
   * for a Front overlap the loop is `for (i = 0; i < 0; i++)` → never executes.
   * Fix: use computeOverlapIds instead of computeActionableIds.
   *
   * The overlap pairs import shots 0..2 with target shots 7..9. Every value in
   * a pair differs between the two sides, so a copy or an average that takes the
   * wrong side, or the wrong shot, shows.
   */

  const importStationIndex = 0;
  const matchedShots = 3;
  const existingStationIndex = 7;
  const direction = 'forward' as const;
  const mergeImportStation = importStationIndex; // default

  const importOverlap = [
    { azimuth_in: 100, azimuth_out: 101, length: 3.0, depth_in: 4.0, depth_out: 4.5 },
    { azimuth_in: 115, azimuth_out: 116, length: 4.0, depth_in: 4.5, depth_out: 5.5 },
    { azimuth_in: 130, azimuth_out: 131, length: 2.5, depth_in: 5.5, depth_out: 6.0 },
  ];
  const targetOverlap = [
    { azimuth_in: 104, azimuth_out: 105, length: 3.4, depth_in: 4.2, depth_out: 4.8 },
    { azimuth_in: 111, azimuth_out: 112, length: 3.6, depth_in: 4.8, depth_out: 5.2 },
    { azimuth_in: 134, azimuth_out: 135, length: 2.9, depth_in: 5.2, depth_out: 6.4 },
  ];

  // The three overlap shots, then six past the junction.
  const importShots: EditableShotLike[] = [
    ...importOverlap.map((values, i) => makeEditableShot(i, values)),
    ...Array.from({ length: 6 }, (_, i) =>
      makeEditableShot(i + 3, { azimuth_in: 140 + i * 4, azimuth_out: 141 + i * 4, length: 2 + i * 0.5 }),
    ),
  ];

  // 15 shots; only 7..9, the overlap, carry the chosen values.
  const targetShots: EditableShotLike[] = Array.from({ length: 15 }, (_, i) =>
    makeEditableShot(
      i,
      targetOverlap[i - existingStationIndex] ?? { azimuth_in: 60 + i * 2, azimuth_out: 61 + i * 2, length: 6 + i * 0.2 },
    ),
  );

  /** The target section after each overlap shot is folded in by `fold` (the handleConfirm loop). */
  function foldOverlap(fold: (target: EditableShotLike, importAsTarget: TargetShotLike) => EditableShotLike) {
    const allOverlap = computeOverlapIds(importStationIndex, matchedShots);
    const shouldReverse = needsDirectionReversal(importStationIndex, direction);
    const overlapParams: OverlapParams = { importStationIndex, matchedShots, existingStationIndex, direction };

    const modifiedTarget = targetShots.map(s => ({ ...s }));
    let modified = false;

    for (const overlapId of allOverlap) {
      const importShot = importShots.find(s => s.id === overlapId);
      if (!importShot) continue;

      const targetShotIdx = computeTargetShotIndex(overlapId, overlapParams);
      if (targetShotIdx < 0 || targetShotIdx >= modifiedTarget.length) continue;

      const targetShot = modifiedTarget[targetShotIdx]!;
      const importAsTarget: TargetShotLike = shouldReverse
        ? reverseTargetShot(importShot)
        : importShot;

      modifiedTarget[targetShotIdx] = fold(targetShot, importAsTarget);
      modified = true;
    }
    return { modifiedTarget, modified, shouldReverse };
  }

  it('computeActionableIds returns EMPTY with default merge station (the bug)', () => {
    const actionable = computeActionableIds(importStationIndex, matchedShots, mergeImportStation);
    expect(actionable.size).toBe(0);
  });

  it('computeOverlapIds returns the correct overlap shots (the fix)', () => {
    const allOverlap = computeOverlapIds(importStationIndex, matchedShots);
    expect(allOverlap).toEqual(new Set([0, 1, 2]));
  });

  it('Front + forward: no direction reversal needed', () => {
    expect(needsDirectionReversal(importStationIndex, direction)).toBe(false);
  });

  it('target shot indices map correctly: import 0→target 7, 1→target 8, 2→target 9', () => {
    const overlapParams: OverlapParams = { importStationIndex, matchedShots, existingStationIndex, direction };
    expect(computeTargetShotIndex(0, overlapParams)).toBe(7);
    expect(computeTargetShotIndex(1, overlapParams)).toBe(8);
    expect(computeTargetShotIndex(2, overlapParams)).toBe(9);
  });

  it('import_data mode: target shots 7..9 get import values', () => {
    const { modifiedTarget, modified, shouldReverse } = foldOverlap(applyTargetData);

    expect(modified).toBe(true);
    expect(shouldReverse).toBe(false);

    // Target shot 7 + k now carries import shot k's values
    importOverlap.forEach((values, k) => {
      const shot = modifiedTarget[existingStationIndex + k]!;
      expect(shot.azimuth_in).toBe(values.azimuth_in);
      expect(shot.length).toBe(values.length);
      expect(shot.depth_in).toBe(values.depth_in);
    });

    // Non-overlap target shots should be unchanged
    expect(modifiedTarget[6]!.azimuth_in).toBe(targetShots[6]!.azimuth_in);
    expect(modifiedTarget[10]!.azimuth_in).toBe(targetShots[10]!.azimuth_in);
  });

  it('interpolate mode: target shots 7..9 get averaged values', () => {
    const { modifiedTarget, modified } = foldOverlap(interpolateShots);

    expect(modified).toBe(true);

    // Target shot 7 + k: the average of its own length and import shot k's
    importOverlap.forEach((values, k) => {
      const own = targetOverlap[k]!;
      expect(modifiedTarget[existingStationIndex + k]!.length).toBeCloseTo((own.length + values.length) / 2, 5);
    });

    // Non-overlap target shots should be unchanged
    expect(modifiedTarget[6]!.length).toBe(targetShots[6]!.length);
    expect(modifiedTarget[10]!.length).toBe(targetShots[10]!.length);
  });

  it('import overlap shots 0..2 properly marked deleted', () => {
    const allOverlap = computeOverlapIds(importStationIndex, matchedShots);

    const deletedImport = importShots.map(shot =>
      (!shot.isNew && allOverlap.has(shot.id))
        ? { ...shot, isDeleted: true }
        : shot,
    );

    expect(deletedImport[0]!.isDeleted).toBe(true);
    expect(deletedImport[1]!.isDeleted).toBe(true);
    expect(deletedImport[2]!.isDeleted).toBe(true);
    expect(deletedImport[3]!.isDeleted).toBeFalsy();

    const filtered = deletedImport.filter(s => !s.isDeleted);
    expect(filtered).toHaveLength(6);
  });
});

// #576 D12: the offset a suggestion opens with, shared by both placement dialogs.
describe('defaultMergeOffset', () => {
  const options = [0, 1, 2, 3].map((offset) => ({ offset, importStation: offset, existingStation: 2 + offset }));

  it('opens on the preferred offset when it is on offer', () => {
    expect(defaultMergeOffset(options, 1)).toBe(1);
  });

  it('falls back to the deepest overlap when nothing is preferred', () => {
    expect(defaultMergeOffset(options, null)).toBe(3);
    expect(defaultMergeOffset(options, undefined)).toBe(3);
  });

  it('falls back to the deepest overlap when the preferred offset is not on offer', () => {
    expect(defaultMergeOffset(options, 7)).toBe(3);
  });

  it('has nothing to open with when there is no option', () => {
    expect(defaultMergeOffset([], 1)).toBeNull();
  });
});
