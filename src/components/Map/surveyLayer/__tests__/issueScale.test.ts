/**
 * The per-error Survey Issues levels: scoring against Min / Warn / Max, the
 * palette position (warn at the middle), the worst / average combine, and the
 * defaults reproducing the original threshold scoring.
 */
import { describe, expect, it } from 'vitest';
import {
  defaultIssueLevels,
  hiddenMaskOf,
  issueScaleFrom,
  levelScore,
  METRIC_AZ,
  METRIC_CLOSURE,
  METRIC_LENGTH,
  positionToScore,
  sameIssueScale,
  ScoreCombiner,
  scoreToPosition,
  validLevels,
  withMetricLevels,
} from '../issueScale';
import { metricScore } from '../shotQuality';

const T = { azimuthDiff: 5, pitchDiff: 10, inclinationDepthDiff: 15 };

describe('levelScore', () => {
  const l = { min: 1, warn: 4, max: 10 };
  it('is 0 at or below Min, 1 at Warn, 3 at or above Max, linear between', () => {
    expect(levelScore(0, l)).toBe(0);
    expect(levelScore(1, l)).toBe(0);
    expect(levelScore(2.5, l)).toBeCloseTo(0.5, 10);
    expect(levelScore(4, l)).toBe(1);
    expect(levelScore(7, l)).toBeCloseTo(2, 10);
    expect(levelScore(10, l)).toBe(3);
    expect(levelScore(99, l)).toBe(3);
  });
  it('ignores the sign and reads NaN as agreement', () => {
    expect(levelScore(-7, l)).toBeCloseTo(2, 10);
    expect(levelScore(NaN, l)).toBe(0);
  });
  it('with the default levels is the original score, min(|v| / threshold, 3)', () => {
    const az = defaultIssueLevels(T).azimuth;
    for (const v of [0, 0.3, 2.5, 5, 5.0001, 7.5, 10, 14.99, 15, 40, -12]) {
      const original = Math.min(Math.abs(v) / T.azimuthDiff, 3);
      expect(levelScore(v, az)).toBeCloseTo(original, 12);
      expect(metricScore(v, T.azimuthDiff)).toBeCloseTo(original, 12);
    }
  });
  it('a zero threshold makes any disagreement the maximum', () => {
    const zero = defaultIssueLevels({ ...T, azimuthDiff: 0 }).azimuth;
    expect(levelScore(0, zero)).toBe(0);
    expect(levelScore(0.01, zero)).toBe(3);
  });
});

describe('palette position', () => {
  it('puts Warn at the middle and round-trips', () => {
    expect(scoreToPosition(0)).toBe(0);
    expect(scoreToPosition(0.5)).toBe(0.25);
    expect(scoreToPosition(1)).toBe(0.5);
    expect(scoreToPosition(2)).toBe(0.75);
    expect(scoreToPosition(3)).toBe(1);
    expect(scoreToPosition(9)).toBe(1);
    expect(scoreToPosition(NaN)).toBe(0);
    for (const s of [0, 0.2, 1, 1.7, 3]) expect(positionToScore(scoreToPosition(s))).toBeCloseTo(s, 12);
  });
});

describe('ScoreCombiner', () => {
  it('worst takes the maximum', () => {
    const c = new ScoreCombiner('worst');
    [0.2, 2.5, 1].forEach((s) => c.add(s));
    expect(c.result()).toBe(2.5);
  });
  it('average takes the mean palette position', () => {
    const c = new ScoreCombiner('average');
    c.add(3);
    c.add(0);
    expect(c.result()).toBe(1); // positions 1 and 0 -> 0.5 -> the warn level
    c.reset();
    c.add(1);
    c.add(1);
    expect(c.result()).toBe(1);
    c.reset();
    expect(c.result()).toBe(0);
  });
});

describe('issueScaleFrom', () => {
  it('defaults: warn on the project thresholds, length 1 m / 3 m, closure 1% / 5%', () => {
    const s = issueScaleFrom(T, {}, [], 'worst');
    expect(s.levels.azimuth).toEqual({ min: 0, warn: 5, max: 15 });
    expect(s.levels.inclination).toEqual({ min: 0, warn: 10, max: 30 });
    expect(s.levels.inclinationDepth).toEqual({ min: 0, warn: 15, max: 45 });
    expect(s.levels.length).toEqual({ min: 0, warn: 1, max: 3 });
    expect(s.levels.closure).toEqual({ min: 0, warn: 1, max: 5 });
    expect(s.hiddenMask).toBe(0);
  });
  it('takes valid overrides only, and hides by id', () => {
    const s = issueScaleFrom(
      T,
      { azimuth: { min: 1, warn: 2, max: 3 }, length: { min: 2, warn: 1, max: 3 } },
      ['length', 'closure', 'nonsense'],
      'average',
    );
    expect(s.levels.azimuth).toEqual({ min: 1, warn: 2, max: 3 });
    expect(s.levels.length).toEqual({ min: 0, warn: 1, max: 3 });
    expect(s.hiddenMask).toBe(METRIC_LENGTH | METRIC_CLOSURE);
    expect(s.combine).toBe('average');
  });
  it('compares by value', () => {
    const a = issueScaleFrom(T, {}, ['azimuth'], 'worst');
    expect(sameIssueScale(a, issueScaleFrom({ ...T }, {}, ['azimuth'], 'worst'))).toBe(true);
    expect(sameIssueScale(a, issueScaleFrom(T, {}, [], 'worst'))).toBe(false);
    expect(sameIssueScale(a, issueScaleFrom(T, {}, ['azimuth'], 'average'))).toBe(false);
    expect(sameIssueScale(a, withMetricLevels(a, 'closure', { min: 0, warn: 2, max: 5 }))).toBe(false);
  });
});

describe('validLevels / hiddenMaskOf', () => {
  it('needs 0 <= min < warn < max, finite', () => {
    expect(validLevels({ min: 0, warn: 1, max: 2 })).toBe(true);
    expect(validLevels({ min: -1, warn: 1, max: 2 })).toBe(false);
    expect(validLevels({ min: 1, warn: 1, max: 2 })).toBe(false);
    expect(validLevels({ min: 0, warn: 2, max: 2 })).toBe(false);
    expect(validLevels({ min: 0, warn: 1, max: Infinity })).toBe(false);
  });
  it('maps ids to bits', () => {
    expect(hiddenMaskOf(['azimuth', 'azimuth'])).toBe(METRIC_AZ);
    expect(hiddenMaskOf([])).toBe(0);
  });
});
