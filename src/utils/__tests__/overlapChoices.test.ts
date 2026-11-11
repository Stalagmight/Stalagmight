/**
 * #681: per-shot choices for repeated survey.
 *
 * INJECTION (tried): `resolvedReading` returning the existing reading for
 * 'use_new' reds "Use new copies the new reading"; dropping the `+ 180 ... - 180`
 * wrap in `readingDiff` reds the 359/1 azimuth case; `>=` in
 * `exceedsRepeatTolerance` reds the at-tolerance case; `toReading(incoming)`
 * for 'use_new' (no fallback) reds "Use new keeps what the new shot lacks";
 * `avgNullable` returning null when either side is null reds "Average keeps what
 * the new shot lacks"; dropping the `sameReading` check reds both "nothing to
 * write" cases; dropping `FLOAT_SLACK` reds "1.0 vs 1.1 and 4.2 vs 4.3" and
 * "255.1 vs 256.1" (one pair of each flips).
 */
import { describe, it, expect } from 'vitest';
import type { TargetShotLike } from '../overlapHelpers';
import {
  exceedsRepeatTolerance,
  readingDiff,
  resolvedReading,
  REPEAT_DIFF_TOLERANCE,
  repeatDiffFlags,
} from '../overlapChoices';

function shot(over: Partial<TargetShotLike> = {}): TargetShotLike {
  return {
    azimuth_in: 90,
    azimuth_out: 90,
    length: 10,
    depth_in: 5,
    depth_out: 6,
    pitch_in: -5,
    pitch_out: -5,
    left: 1,
    right: 1,
    up: 1,
    down: 1,
    ...over,
  };
}

describe('resolvedReading', () => {
  const existing = shot({ length: 10, azimuth_in: 350, azimuth_out: 350, depth_out: 6 });
  const incoming = shot({ length: 12, azimuth_in: 10, azimuth_out: 10, depth_out: 8 });

  it('Keep existing leaves the existing shot alone', () => {
    expect(resolvedReading('keep_existing', existing, incoming)).toBeNull();
  });

  it('Use new copies the new reading, and nothing but the reading', () => {
    const withIdentity = { ...incoming, id: 42, kind: 'Normal', comment: 'x' };
    expect(resolvedReading('use_new', existing, withIdentity)).toEqual(incoming);
  });

  // `updateShot` reads null as CLEAR, so a gap in the new shot must not reach it.
  const gappy = shot({ length: 12, pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null });
  const lrud = shot({ pitch_in: -3, pitch_out: -4, left: 2, right: 3, up: 4, down: 5 });

  it('Use new keeps what the new shot lacks', () => {
    const r = resolvedReading('use_new', lrud, gappy)!;
    expect(r.length).toBe(12);
    expect([r.pitch_in, r.pitch_out, r.left, r.right, r.up, r.down]).toEqual([-3, -4, 2, 3, 4, 5]);
  });

  it('Average keeps what the new shot lacks', () => {
    const r = resolvedReading('average', lrud, gappy)!;
    expect(r.length).toBe(11);
    expect([r.pitch_in, r.pitch_out, r.left, r.right, r.up, r.down]).toEqual([-3, -4, 2, 3, 4, 5]);
  });

  it('an identical repeat leaves nothing to write, for Use new and Average', () => {
    const odd = shot({ azimuth_in: 123.4, azimuth_out: 303.4, length: 7.3, depth_in: 4.2, depth_out: 4.3 });
    expect(resolvedReading('use_new', odd, { ...odd })).toBeNull();
    expect(resolvedReading('average', odd, { ...odd })).toBeNull();
  });

  it('a repeat that differs only where the new shot has no value leaves nothing to write', () => {
    const sameButGappy = { ...lrud, pitch_in: null, left: null };
    expect(resolvedReading('use_new', lrud, sameButGappy)).toBeNull();
    expect(resolvedReading('average', lrud, sameButGappy)).toBeNull();
  });

  it('Average takes the mean, circular for azimuth', () => {
    const r = resolvedReading('average', existing, incoming)!;
    expect(r.length).toBeCloseTo(11);
    expect(r.depth_out).toBeCloseTo(7);
    // 350 and 10 average to north, not south.
    expect(Math.min(r.azimuth_in, 360 - r.azimuth_in)).toBeCloseTo(0);
    expect(Object.keys(r)).not.toContain('id');
  });
});

describe('readingDiff / exceedsRepeatTolerance', () => {
  it('an identical repeat differs by nothing and is not flagged', () => {
    const d = readingDiff(shot(), shot());
    expect(d).toEqual({ length: 0, azimuth: 0, inclination: 0, depth: 0 });
    expect(exceedsRepeatTolerance(d)).toBe(false);
  });

  it('azimuth difference wraps through north', () => {
    const d = readingDiff(shot({ azimuth_in: 359, azimuth_out: 359 }), shot({ azimuth_in: 1, azimuth_out: 1 }));
    expect(d.azimuth).toBeCloseTo(2);
    expect(exceedsRepeatTolerance(d)).toBe(true);
  });

  it('depth difference is the larger of the start and end differences', () => {
    const d = readingDiff(shot({ depth_in: 5, depth_out: 6 }), shot({ depth_in: 5.05, depth_out: 6.3 }));
    expect(d.depth).toBeCloseTo(0.3);
  });

  it('inclination is null when either reading has none', () => {
    const d = readingDiff(shot(), shot({ pitch_in: null, pitch_out: null }));
    expect(d.inclination).toBeNull();
    expect(exceedsRepeatTolerance(d)).toBe(false);
  });

  it('a difference exactly at the tolerance is not flagged; just over it is', () => {
    const at = { length: REPEAT_DIFF_TOLERANCE.length, azimuth: 0, inclination: 0, depth: 0 };
    expect(exceedsRepeatTolerance(at)).toBe(false);
    expect(exceedsRepeatTolerance({ ...at, length: REPEAT_DIFF_TOLERANCE.length + 0.01 })).toBe(true);
  });

  // Readings are binary floats: 1.1 - 1.0 is just over 0.1, 4.3 - 4.2 just under.
  it('a difference of exactly the tolerance classifies alike: 1.0 vs 1.1 and 4.2 vs 4.3', () => {
    for (const [a, b] of [[1.0, 1.1], [4.2, 4.3]] as const) {
      expect(exceedsRepeatTolerance(readingDiff(shot({ length: a }), shot({ length: b })))).toBe(false);
      const depth = readingDiff(shot({ depth_in: a, depth_out: a }), shot({ depth_in: b, depth_out: b }));
      expect(exceedsRepeatTolerance(depth)).toBe(false);
      expect(repeatDiffFlags(depth).depth).toBe(false);
    }
  });

  // Without the slack 255.1 vs 256.1 is flagged and 255.4 vs 256.4 is not.
  it('a 1 degree azimuth difference classifies alike: 255.1 vs 256.1, 255.4 vs 256.4, 39.1 vs 40.1', () => {
    for (const [a, b] of [[255.1, 256.1], [255.4, 256.4], [39.1, 40.1]] as const) {
      const d = readingDiff(shot({ azimuth_in: a, azimuth_out: a }), shot({ azimuth_in: b, azimuth_out: b }));
      expect(exceedsRepeatTolerance(d)).toBe(false);
      expect(repeatDiffFlags(d).azimuth).toBe(false);
    }
  });
});
