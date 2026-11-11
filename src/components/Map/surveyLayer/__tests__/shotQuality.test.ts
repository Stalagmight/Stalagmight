import { describe, expect, it } from 'vitest';
import {
  computeShotQualityMetrics,
  metricScore,
  SCORE_MAX,
  shotMetricScore,
  METRIC_AZ,
  METRIC_INCL_DEPTH,
  METRIC_PITCH,
} from '../shotQuality';
import { surveyDataWithAdjustedStations } from '../../../../types/survey-data';
import { buildSurveyData, oneShot, type ShotReadings } from './surveyQualityFixture';

const metricsOf = (r: ShotReadings) => {
  const m = computeShotQualityMetrics(oneShot(r));
  return {
    az: m.azDelta[0]!,
    pitch: m.pitchDelta[0]!,
    dep: m.inclDepthDelta[0]!,
    len: m.lengthDelta[0]!,
  };
};

describe('metricScore', () => {
  it('is 0 for agreement, 1 at the threshold, capped at 3', () => {
    expect(metricScore(0, 20)).toBe(0);
    expect(metricScore(10, 20)).toBe(0.5);
    expect(metricScore(20, 20)).toBe(1);
    expect(metricScore(-40, 20)).toBe(2);
    expect(metricScore(60, 20)).toBe(SCORE_MAX);
    expect(metricScore(500, 20)).toBe(SCORE_MAX);
  });
  it('scores NaN as 0, and any delta as the maximum when the threshold is 0', () => {
    expect(metricScore(NaN, 20)).toBe(0);
    expect(metricScore(0.1, 0)).toBe(SCORE_MAX);
    expect(metricScore(0, 0)).toBe(0);
  });
});

/**
 * Parity with the Rust validator's own test cases (`core::validation`): a
 * metric scores above 1 exactly when the validator warns. The Rust tests use
 * threshold 20 for azimuth (5 and 60 in the strict / lenient variants) and 10
 * for inclination vs depth; every value here is exact in Float32.
 */
describe('parity with the validator: score > 1 when it warns (exact for these f32-exact readings)', () => {
  const azCases: [number, number, number, boolean][] = [
    [45, 45, 20, false],
    [45, 50, 20, false],
    [45, 65, 20, false], // exactly at the threshold is not a warning
    [45, 70, 20, true],
    [355, 5, 20, false],
    [355, 20, 20, true],
    [90, 93, 5, false],
    [90, 100, 5, true],
    [0, 50, 60, false],
    [45, 180, 20, true],
  ];
  it.each(azCases)('azimuth %s / %s at threshold %s warns: %s', (azIn, azOut, t, warns) => {
    expect(metricScore(metricsOf({ azimuthIn: azIn, azimuthOut: azOut }).az, t) > 1).toBe(warns);
  });

  // length 10, depth 0 -> 5 implies +30 (+down); threshold 10.
  const down = { length: 10, depthIn: 0, depthOut: 5 };
  const depCases: [string, ShotReadings, boolean][] = [
    ['+30 on a descending shot', { ...down, pitchIn: 30, pitchOut: 30 }, false],
    ['-30 on a descending shot (reversed)', { ...down, pitchIn: -30, pitchOut: -30 }, true],
    ['-10', { ...down, pitchIn: -10, pitchOut: -10 }, true],
    ['+10', { ...down, pitchIn: 10, pitchOut: 10 }, true],
    ['+80', { ...down, pitchIn: 80, pitchOut: 80 }, true],
    ['-30 on an ascending shot', { length: 10, depthIn: 5, depthOut: 0, pitchIn: -30, pitchOut: -30 }, false],
    ['+30 on an ascending shot', { length: 10, depthIn: 5, depthOut: 0, pitchIn: 30, pitchOut: 30 }, true],
    ['a vertical shot at +90', { length: 5, depthIn: 0, depthOut: 5, pitchIn: 90, pitchOut: 90 }, false],
    ['a recorded 0 with a real depth change', { ...down, pitchIn: 0, pitchOut: 0 }, true],
  ];
  it.each(depCases)('inclination vs depth: %s warns: %s', (_name, r, warns) => {
    expect(metricScore(metricsOf(r).dep, 10) > 1).toBe(warns);
  });

  it('inclination in/out at threshold 5 (test_pitch_deviation)', () => {
    const pitch = (pIn: number | null, pOut: number | null) => metricsOf({ pitchIn: pIn, pitchOut: pOut }).pitch;
    expect(metricScore(pitch(2, 2), 5) > 1).toBe(false);
    expect(metricScore(pitch(2, 7), 5) > 1).toBe(false);
    expect(metricScore(pitch(2, 8), 5) > 1).toBe(true);
    expect(metricScore(pitch(-3, 3), 5) > 1).toBe(true);
    expect(pitch(2, null)).toBeNaN();
    expect(pitch(null, 8)).toBeNaN();
    expect(pitch(0, 120)).toBeNaN();
  });
});

describe('computeShotQualityMetrics', () => {
  it('wraps the azimuth difference into [0, 180]', () => {
    expect(metricsOf({ azimuthIn: 0, azimuthOut: 180 }).az).toBe(180);
    expect(metricsOf({ azimuthIn: 10, azimuthOut: 350 }).az).toBe(20);
    expect(metricsOf({ azimuthIn: 350, azimuthOut: 10 }).az).toBe(20);
    expect(metricsOf({ azimuthIn: 400, azimuthOut: 30 }).az).toBe(10);
    expect(metricsOf({ azimuthIn: NaN, azimuthOut: 30 }).az).toBeNaN();
  });

  it('uses the one recorded inclination leg for inclination vs depth', () => {
    const m = metricsOf({ length: 10, depthIn: 0, depthOut: 5, pitchIn: 30, pitchOut: null });
    expect(m.dep).toBeCloseTo(0, 4);
    expect(m.pitch).toBeNaN();
  });

  it('averages opposite legs (mean 0) and reports their whole difference', () => {
    const m = metricsOf({ length: 10, pitchIn: -80, pitchOut: 80 });
    expect(m.pitch).toBe(160);
    expect(m.dep).toBe(0);
  });

  it('withholds inclination vs depth on a shot shorter than its depth change, like the validator', () => {
    const m = metricsOf({ length: 2, depthIn: 0, depthOut: 5, pitchIn: 30, pitchOut: 30 });
    expect(m.dep).toBeNaN();
    // ... while the length delta still says how much is missing.
    expect(m.len).toBeCloseTo(2 - 10, 4);
    // A vertical shot (length == depth change) is still checked.
    expect(metricsOf({ length: 5, depthIn: 0, depthOut: 5, pitchIn: 90, pitchOut: 90 }).dep).toBeCloseTo(0, 4);
  });

  it('withholds the inclination metrics with no inclination, a leg out of range, or no length', () => {
    expect(metricsOf({ depthOut: 5 }).dep).toBeNaN();
    expect(metricsOf({ depthOut: 5, pitchIn: 120, pitchOut: 120 }).dep).toBeNaN();
    expect(metricsOf({ length: 0, depthOut: 0, pitchIn: 10, pitchOut: 10 }).dep).toBeNaN();
  });

  it('signs inclination vs depth as recorded minus implied', () => {
    expect(metricsOf({ length: 10, depthIn: 0, depthOut: 5, pitchIn: 40, pitchOut: 40 }).dep).toBeCloseTo(10, 4);
    expect(metricsOf({ length: 10, depthIn: 0, depthOut: 5, pitchIn: 20, pitchOut: 20 }).dep).toBeCloseTo(-10, 4);
  });

  it('length delta is recorded minus |dz| / |sin(inclination)|', () => {
    expect(metricsOf({ length: 10, depthIn: 0, depthOut: 5, pitchIn: 30, pitchOut: 30 }).len).toBeCloseTo(0, 4);
    // 5 / sin 20 = 14.619 -> 10 - 14.619
    expect(metricsOf({ length: 10, depthIn: 0, depthOut: 5, pitchIn: 20, pitchOut: 20 }).len).toBeCloseTo(-4.619, 3);
  });

  it('withholds the length delta on a flat, shallow, reversed or level shot', () => {
    expect(metricsOf({ length: 10, depthIn: 0, depthOut: 0.2, pitchIn: 2, pitchOut: 2 }).len).toBeNaN();
    // 14.9° is below the 15° floor (a 1° error would swing the length by ~0.7 m);
    // 15° is not.
    expect(metricsOf({ length: 10, depthIn: 0, depthOut: 2.5, pitchIn: 14.9, pitchOut: 14.9 }).len).toBeNaN();
    expect(metricsOf({ length: 10, depthIn: 0, depthOut: 2.5, pitchIn: 15, pitchOut: 15 }).len).toBeCloseTo(0.341, 3);
    expect(metricsOf({ length: 10, depthIn: 0, depthOut: 5, pitchIn: -30, pitchOut: -30 }).len).toBeNaN();
    expect(metricsOf({ length: 10, depthIn: 3, depthOut: 3, pitchIn: 30, pitchOut: 30 }).len).toBeNaN();
  });

  it('is shared by the Raw Data and Closures Computed views (same shot columns)', () => {
    const data = buildSurveyData({
      sections: [{ id: 1, shotIds: [101] }],
      stations: [
        { sectionId: 1, stationIndex: 0 },
        { sectionId: 1, stationIndex: 1 },
      ],
    });
    const adjusted = surveyDataWithAdjustedStations(data, []);
    expect(computeShotQualityMetrics(adjusted)).toBe(computeShotQualityMetrics(data));
  });
});

describe('shotMetricScore', () => {
  const m = computeShotQualityMetrics(
    oneShot({ azimuthIn: 0, azimuthOut: 30, length: 10, depthIn: 0, depthOut: 5, pitchIn: 20, pitchOut: 40 }),
  );
  const t = { azimuthDiff: 20, pitchDiff: 10, inclinationDepthDiff: 15 };
  it('takes the worst metric', () => {
    // az 30/20 = 1.5, pitch 20/10 = 2, incl/depth |30-30| = 0
    expect(shotMetricScore(m, 0, t, 0)).toBeCloseTo(2, 5);
  });
  it('skips the ignored metrics', () => {
    expect(shotMetricScore(m, 0, t, METRIC_PITCH)).toBeCloseTo(1.5, 5);
    expect(shotMetricScore(m, 0, t, METRIC_PITCH | METRIC_AZ)).toBeCloseTo(0, 4);
    expect(shotMetricScore(m, 0, t, METRIC_AZ | METRIC_PITCH | METRIC_INCL_DEPTH)).toBe(0);
  });
});
