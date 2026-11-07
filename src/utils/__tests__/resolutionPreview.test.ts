import { describe, it, expect } from 'vitest';
import {
  computeGhostEnd,
  horizontalLength,
  lengthFromInclination,
  lengthFixableFromInclination,
  exceedsSlopeLength,
} from '../resolutionPreview';

describe('lengthFixableFromInclination (#57 / Codex)', () => {
  // dz = depth_out - depth_in. +down convention: descending dz>0 needs pitch>0,
  // ascending dz<0 needs pitch<0; a positive length only exists when the recorded
  // pitch points the same way as the depth change.
  it('true when the recorded pitch matches the depth direction', () => {
    expect(lengthFixableFromInclination(5, 30)).toBe(true); // descent, pitch down
    expect(lengthFixableFromInclination(-5, -30)).toBe(true); // ascent, pitch up
  });
  it('false for a reversed-sign inclination (no positive length can fix it)', () => {
    expect(lengthFixableFromInclination(5, -30)).toBe(false); // descent recorded as -30
    expect(lengthFixableFromInclination(-5, 30)).toBe(false); // ascent recorded as +30
  });
  it('false for zero depth change and for null/too-shallow pitch', () => {
    expect(lengthFixableFromInclination(0, 30)).toBe(false);
    expect(lengthFixableFromInclination(5, null)).toBe(false);
    expect(lengthFixableFromInclination(5, -2)).toBe(false); // |sin| <= floor
  });
});

describe('horizontalLength', () => {
  it('projects slope length by depth change', () => {
    expect(horizontalLength(10, 0)).toBeCloseTo(10, 6);
    expect(horizontalLength(5, 3)).toBeCloseTo(4, 6); // 3-4-5
    expect(horizontalLength(5, 5)).toBeCloseTo(0, 6); // vertical
    expect(horizontalLength(5, 9)).toBe(0); // clamped, never NaN
  });

  it('stays finite for a tape so long its square overflows (#576 NF-W4M-5)', () => {
    // 1e200 squared is Infinity, and Infinity - Infinity is NaN. The backend's
    // `horizontal_length_from_slope` scales both into [0, 1] first, so a
    // 10:6 tape-to-drop ratio still projects to 8 - the ghost must agree.
    expect(horizontalLength(1e200, 6e199) / 8e199).toBeCloseTo(1, 12);
    expect(horizontalLength(-1e200, -6e199) / 8e199).toBeCloseTo(1, 12);
    expect(horizontalLength(1e200, 2e200)).toBe(0); // steeper than vertical, clamped
  });
});

describe('computeGhostEnd', () => {
  // A due-east shot: azimuth 90° places the end at +x (dx = horiz·sin90 = horiz).
  const start = { x: 0, y: 0 };
  const eastEnd = { x: 10, y: 0 };

  it('returns the endpoint unchanged when the proposal matches the current', () => {
    const g = computeGhostEnd(start, eastEnd, 90, 10, 90, 10);
    expect(g).not.toBeNull();
    expect(g!.x).toBeCloseTo(10, 6);
    expect(g!.y).toBeCloseTo(0, 6);
  });

  it('rotates the endpoint about the start when azimuth changes', () => {
    // East (90°) -> North (0°): end moves from (10,0) to (0,10).
    const g = computeGhostEnd(start, eastEnd, 90, 10, 0, 10);
    expect(g).not.toBeNull();
    expect(g!.x).toBeCloseTo(0, 6);
    expect(g!.y).toBeCloseTo(10, 6);
  });

  it('scales along the bearing when length changes', () => {
    // East shot lengthened 10 -> 15: end moves to (15, 0).
    const g = computeGhostEnd(start, eastEnd, 90, 10, 90, 15);
    expect(g).not.toBeNull();
    expect(g!.x).toBeCloseTo(15, 6);
    expect(g!.y).toBeCloseTo(0, 6);
  });

  it('returns null when the planar model cannot reproduce the solved endpoint', () => {
    // Solved end is north, but the shot azimuth says east - a rotated/offset
    // section the model does not capture. No (misleading) ghost.
    const northEnd = { x: 0, y: 10 };
    expect(computeGhostEnd(start, northEnd, 90, 10, 0, 10)).toBeNull();
  });

  it('keeps the ghost anchored to the real end even with float drift within tolerance', () => {
    const almostEast = { x: 10.05, y: 0.0 }; // within 0.1 + 0.05*10 = 0.6
    const g = computeGhostEnd(start, almostEast, 90, 10, 90, 10);
    expect(g).not.toBeNull();
    // Proposal == current, so ghost sits on the real (drifted) end.
    expect(g!.x).toBeCloseTo(10.05, 6);
    expect(g!.y).toBeCloseTo(0, 6);
  });
});

describe('lengthFromInclination', () => {
  it('returns null when there is no recorded pitch', () => {
    expect(lengthFromInclination(5, null)).toBeNull();
  });

  it('returns null when the inclination is below the sin floor', () => {
    // pitch ≈ 2.8° -> sin ≈ 0.0488 < 0.05 (MIN_SIN_INCLINATION) -> withheld.
    expect(lengthFromInclination(5, 2.8)).toBeNull();
  });

  it('returns |dz| / |sin(pitch)| once above the floor', () => {
    // pitch = 3° -> sin ≈ 0.05234 > 0.05 -> computes.
    expect(lengthFromInclination(5, 3)).toBeCloseTo(5 / Math.sin((3 * Math.PI) / 180), 6);
    // pitch = 30° -> sin = 0.5 -> length = 3 / 0.5 = 6.
    expect(lengthFromInclination(3, 30)).toBeCloseTo(6, 6);
    // pitch = 90° -> sin = 1 -> length = |dz|.
    expect(lengthFromInclination(5, 90)).toBeCloseTo(5, 6);
  });

  it('ignores the sign of the pitch (uses magnitudes)', () => {
    expect(lengthFromInclination(4, 30)).toBeCloseTo(lengthFromInclination(4, -30)!, 9);
  });

  it('ignores the sign of the depth change and is non-negative', () => {
    const up = lengthFromInclination(-4, -90);
    expect(up).toBeCloseTo(4, 6);
    expect(up!).toBeGreaterThanOrEqual(0);
  });

  it('returns 0 for a zero depth change at a valid inclination', () => {
    expect(lengthFromInclination(0, 90)).toBe(0);
  });
});

describe('exceedsSlopeLength', () => {
  it('is true when the length is shorter than the depth change', () => {
    expect(exceedsSlopeLength(5, 0, 12)).toBe(true); // |dz| = 12 > 5
  });

  it('is false when the length comfortably spans the depth change', () => {
    expect(exceedsSlopeLength(5, 0, 3)).toBe(false); // |dz| = 3 < 5
  });

  it('uses a 1e-6 epsilon at the boundary, matching the backend check', () => {
    // length == |dz| exactly: not exceeding (a vertical shot is valid).
    expect(exceedsSlopeLength(5, 0, 5)).toBe(false);
    // Just over the epsilon: exceeds.
    expect(exceedsSlopeLength(5, 0, 5.001)).toBe(true);
    // Within epsilon: still not exceeding.
    expect(exceedsSlopeLength(5, 0, 5 + 1e-7)).toBe(false);
  });

  it('is false for a non-positive length or non-finite depths', () => {
    expect(exceedsSlopeLength(0, 0, 12)).toBe(false);
    expect(exceedsSlopeLength(-5, 0, 12)).toBe(false);
    expect(exceedsSlopeLength(5, NaN, 12)).toBe(false);
    expect(exceedsSlopeLength(5, 0, NaN)).toBe(false);
  });
});
