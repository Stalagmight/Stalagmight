import { describe, it, expect } from 'vitest';
import { buildShotUpdates, buildShotUpdateBatch, AZIMUTH_AVG_KEY, PITCH_AVG_KEY } from '../dataTableEdits';
import type { Shot } from '../../../types/project';
import { makeShot } from '../../../test/factories/survey';

const shot = (over: Partial<Shot> = {}): Shot =>
  makeShot({ azimuth_in: 90, azimuth_out: 90, depth_out: 1, ...over });

describe('buildShotUpdates', () => {
  it('maps real column keys straight through', () => {
    const u = buildShotUpdates(shot(), [
      { colKey: 'length', value: 12.5 },
      { colKey: 'depth_out', value: 2 },
      { colKey: 'left', value: null },
      { colKey: 'comment', value: 'note' },
    ]);
    expect(u).toEqual({ length: 12.5, depth_out: 2, left: null, comment: 'note' });
  });

  it('expands the azimuth-average column into both legs via the delta', () => {
    // mean of (80,100) = 90; new mean 95 → +5 to each leg.
    const u = buildShotUpdates(shot({ azimuth_in: 80, azimuth_out: 100 }), [
      { colKey: AZIMUTH_AVG_KEY, value: 95 },
    ]);
    expect(u.azimuth_in).toBeCloseTo(85, 6);
    expect(u.azimuth_out).toBeCloseTo(105, 6);
  });

  it('rotates opposite legs from the backend mean, not the unit-vector one (#576 D14)', () => {
    // Legs 100/280 are a blunder pair whose backend mean is 10 (the tie goes to
    // in - 90). The unit-vector mean put it at 198.43, so typing 12 into the Avg
    // cell moved both legs by -186.43 instead of +2.
    const u = buildShotUpdates(shot({ azimuth_in: 100, azimuth_out: 280 }), [
      { colKey: AZIMUTH_AVG_KEY, value: 12 },
    ]);
    expect(u.azimuth_in).toBeCloseTo(102, 9);
    expect(u.azimuth_out).toBeCloseTo(282, 9);
  });

  it('skips a non-finite azimuth average', () => {
    expect(buildShotUpdates(shot(), [{ colKey: AZIMUTH_AVG_KEY, value: NaN }])).toEqual({});
  });

  it('wraps azimuths across 0/360', () => {
    // mean of (350,10) = 0; new mean 5 → +5 → in 355, out 15.
    const u = buildShotUpdates(shot({ azimuth_in: 350, azimuth_out: 10 }), [
      { colKey: AZIMUTH_AVG_KEY, value: 5 },
    ]);
    expect(u.azimuth_in).toBeCloseTo(355, 6);
    expect(u.azimuth_out).toBeCloseTo(15, 6);
  });

  it('stores due north as 0, not 360, when the typed mean is 0', () => {
    // circularMeanDegrees(1.1, 1.1) is 1.1000000000000005, so the delta leaves
    // each leg at about -4.4e-16, which `+ 360` rounded to exactly 360: an
    // out-of-range bearing the backend flags as a non-ignorable INVALID_AZIMUTH.
    const u = buildShotUpdates(shot({ azimuth_in: 1.1, azimuth_out: 1.1 }), [
      { colKey: AZIMUTH_AVG_KEY, value: 0 },
    ]);
    expect(u).toEqual({ azimuth_in: 0, azimuth_out: 0 });
  });

  it('shifts both pitch legs by the delta when both are present', () => {
    // mean of (10,30) = 20; new mean 25 → +5 to each leg.
    const u = buildShotUpdates(shot({ pitch_in: 10, pitch_out: 30 }), [
      { colKey: PITCH_AVG_KEY, value: 25 },
    ]);
    expect(u.pitch_in).toBeCloseTo(15, 6);
    expect(u.pitch_out).toBeCloseTo(35, 6);
  });

  it('sets the single present pitch leg to the typed mean', () => {
    const u = buildShotUpdates(shot({ pitch_in: 10, pitch_out: null }), [
      { colKey: PITCH_AVG_KEY, value: -5 },
    ]);
    expect(u).toEqual({ pitch_in: -5 });
  });

  it('sets both pitch legs when neither is present', () => {
    const u = buildShotUpdates(shot(), [{ colKey: PITCH_AVG_KEY, value: 12 }]);
    expect(u).toEqual({ pitch_in: 12, pitch_out: 12 });
  });

  it('skips a non-finite pitch average', () => {
    expect(buildShotUpdates(shot(), [{ colKey: PITCH_AVG_KEY, value: 'abc' }])).toEqual({});
  });

  it('clears both pitch legs when the average cell is cleared (null)', () => {
    // A cleared Avg cell must clear the inclination, not coerce null → 0.
    const u = buildShotUpdates(shot({ pitch_in: 10, pitch_out: 30 }), [
      { colKey: PITCH_AVG_KEY, value: null },
    ]);
    expect(u).toEqual({ pitch_in: null, pitch_out: null });
  });
});

describe('buildShotUpdateBatch', () => {
  const shots = [shot({ id: 10 }), shot({ id: 11 }), shot({ id: 12 })];

  it('groups edits into one update_shot per row, keyed by shot id', () => {
    const cmds = buildShotUpdateBatch(
      [
        { row: 0, colKey: 'length', value: 5 },
        { row: 0, colKey: 'depth_in', value: 1 },
        { row: 2, colKey: 'comment', value: 'x' },
      ],
      shots,
      7,
    );
    expect(cmds).toEqual([
      { type: 'update_shot', section_id: 7, shot_id: 10, updates: { length: 5, depth_in: 1 } },
      { type: 'update_shot', section_id: 7, shot_id: 12, updates: { comment: 'x' } },
    ]);
  });

  it('drops edits on rows outside the shot list and shots with no net change', () => {
    const cmds = buildShotUpdateBatch(
      [
        { row: 99, colKey: 'length', value: 5 },
        { row: 1, colKey: AZIMUTH_AVG_KEY, value: NaN },
      ],
      shots,
      7,
    );
    expect(cmds).toEqual([]);
  });
});

describe('buildShotUpdates azimuth with a missing leg', () => {
  const base = { azimuth_in: NaN, azimuth_out: NaN, pitch_in: null, pitch_out: null };

  it('sets both legs when neither is measured', () => {
    const u = buildShotUpdates(base, [{ colKey: AZIMUTH_AVG_KEY, value: 45 }]);
    expect(u.azimuth_in).toBeCloseTo(45);
    expect(u.azimuth_out).toBeCloseTo(45);
  });

  it('offsets from the ONE measured leg rather than folding to 0', () => {
    // A shot with a single reading is valid. Averaging against a NaN produced a
    // NaN delta, which normalizeAngle turned into due north.
    const u = buildShotUpdates({ ...base, azimuth_in: 90 }, [{ colKey: AZIMUTH_AVG_KEY, value: 100 }]);
    expect(u.azimuth_in).toBeCloseTo(100);
    expect(u.azimuth_out).toBeCloseTo(100);
  });

  it('works whichever leg is the measured one', () => {
    const u = buildShotUpdates({ ...base, azimuth_out: 270 }, [{ colKey: AZIMUTH_AVG_KEY, value: 260 }]);
    expect(u.azimuth_in).toBeCloseTo(260);
    expect(u.azimuth_out).toBeCloseTo(260);
  });

  it('never yields 0 from a measured bearing', () => {
    const u = buildShotUpdates({ ...base, azimuth_in: 123 }, [{ colKey: AZIMUTH_AVG_KEY, value: 124 }]);
    expect(u.azimuth_in).not.toBe(0);
    expect(u.azimuth_out).not.toBe(0);
  });
});
