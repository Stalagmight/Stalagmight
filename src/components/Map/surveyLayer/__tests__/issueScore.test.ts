import { describe, expect, it } from 'vitest';
import { computeIssueScores, resolveIssueRender, type IssueRenderInput } from '../issueScore';
import { computeShotQualityMetrics, METRIC_AZ, METRIC_INCL_DEPTH, METRIC_LENGTH, METRIC_PITCH } from '../shotQuality';
import type { ProjectWarning } from '../../../../types/commands';
import type { SurveyData } from '../../../../types/survey-data';
import { buildSurveyData, warn, type ShotReadings } from './surveyQualityFixture';

const T = { azimuthDiff: 20, pitchDiff: 10, inclinationDepthDiff: 15 };

/**
 * Standard fixture:
 *   section 1: shots [101, 102] -> stations idx 0,1,2 (global 0,1,2)
 *   section 2: shot  [201]      -> stations idx 0,1   (global 3,4)
 * shot 101 (g0) connects global 0-1; shot 102 (g1) connects global 1-2;
 * shot 201 (g2) connects global 3-4. Every shot is pristine unless `shots`
 * says otherwise.
 */
function fixture(opts: { merged?: number[][]; shots?: Record<number, ShotReadings> } = {}): SurveyData {
  return buildSurveyData({
    sections: [
      { id: 1, shotIds: [101, 102] },
      { id: 2, shotIds: [201] },
    ],
    stations: [
      { sectionId: 1, stationIndex: 0 },
      { sectionId: 1, stationIndex: 1 },
      { sectionId: 1, stationIndex: 2 },
      { sectionId: 2, stationIndex: 0 },
      { sectionId: 2, stationIndex: 1 },
    ],
    ...(opts.merged ? { merged: opts.merged } : {}),
    ...(opts.shots ? { shots: opts.shots } : {}),
  });
}

function scores(data: SurveyData, warnings: ProjectWarning[] = []) {
  const s = computeIssueScores(data, computeShotQualityMetrics(data), warnings, T);
  return {
    shot: [...s.shotScore].map((v) => +v.toFixed(3)),
    station: [...s.stationScore].map((v) => +v.toFixed(3)),
    label: [...s.labelScore].map((v) => +v.toFixed(3)),
    ignored: [...s.ignored],
  };
}

describe('computeIssueScores: shots', () => {
  it('pristine readings and no warnings score 0 everywhere', () => {
    const s = scores(fixture());
    expect(s.shot).toEqual([0, 0, 0]);
    expect(s.station).toEqual([0, 0, 0, 0, 0]);
  });

  it('grades a shot by how far its readings disagree, relative to the threshold', () => {
    // az delta 10 of 20 -> 0.5; az delta 40 -> 2; pitch delta 40 of 10 -> capped 3.
    const s = scores(
      fixture({
        shots: {
          0: { azimuthIn: 90, azimuthOut: 100 },
          1: { azimuthIn: 90, azimuthOut: 130 },
          2: { pitchIn: -20, pitchOut: 20 },
        },
      }),
    );
    expect(s.shot).toEqual([0.5, 2, 3]);
    expect(s.label).toEqual([0.5, 2, 3]);
  });

  it('an unresolved error forces 3 on colour and label', () => {
    const s = scores(fixture({ shots: { 0: { azimuthIn: 90, azimuthOut: 95 } } }), [warn(1, 101, 'error')]);
    expect(s.shot[0]).toBe(3);
    expect(s.label[0]).toBe(3);
  });

  it('another unresolved warning floors the colour at 1 but not the label', () => {
    const s = scores(fixture({ shots: { 1: { azimuthIn: 90, azimuthOut: 130 } } }), [
      warn(1, 101, 'warning', { code: 'SHORT_SHOT' }),
      warn(1, 102, 'warning', { code: 'LONG_SHOT' }),
    ]);
    expect(s.shot[0]).toBe(1); // pristine readings, floored
    expect(s.label[0]).toBe(0);
    expect(s.shot[1]).toBe(2); // already worse than the floor
  });

  it('a resolved limit warning no longer floors the colour', () => {
    const s = scores(fixture(), [warn(1, 101, 'warning', { code: 'SHORT_SHOT', isResolved: true })]);
    expect(s.shot[0]).toBe(0);
  });

  it('an ignored deviation drops only its own metric', () => {
    const data = fixture({
      shots: { 0: { azimuthIn: 0, azimuthOut: 30, pitchIn: 0, pitchOut: 20, length: 10 } },
    });
    // az 30/20 = 1.5, pitch 20/10 = 2.
    expect(scores(data).shot[0]).toBe(2);
    const ignoredPitch = scores(data, [warn(1, 101, 'warning', { code: 'PITCH_DEVIATION', isResolved: true })]);
    expect(ignoredPitch.shot[0]).toBe(1.5);
    expect(ignoredPitch.ignored[0]).toBe(METRIC_PITCH);
    const ignoredBoth = scores(data, [
      warn(1, 101, 'warning', { code: 'PITCH_DEVIATION', isResolved: true }),
      warn(1, 101, 'warning', { code: 'AZIMUTH_DEVIATION', isResolved: true }),
    ]);
    expect(ignoredBoth.ignored[0]).toBe(METRIC_PITCH | METRIC_AZ);
    // The remaining metric (incl. vs depth: mean 10 vs implied 0) is 10/15.
    expect(ignoredBoth.shot[0]).toBeCloseTo(0.667, 3);
  });

  it('ignoring the inclination-vs-depth mismatch also drops the length delta', () => {
    const s = scores(fixture(), [warn(1, 101, 'warning', { code: 'INCLINATION_DEPTH_MISMATCH', isResolved: true })]);
    expect(s.ignored[0]).toBe(METRIC_INCL_DEPTH | METRIC_LENGTH);
  });

  it('an unknown unresolved code floors at 1; a stale warning is skipped', () => {
    const s = scores(fixture(), [warn(1, 101, 'warning', { code: 'SOMETHING_NEW' }), warn(9, 999, 'error'), warn(1, 999, 'error')]);
    expect(s.shot).toEqual([1, 0, 0]);
  });
});

describe('computeIssueScores: stations', () => {
  it('a station takes the highest score of the shots touching it', () => {
    const s = scores(fixture({ shots: { 0: { azimuthIn: 90, azimuthOut: 100 }, 1: { azimuthIn: 90, azimuthOut: 130 } } }));
    // g0: shot 101 (0.5); g1: shots 101 + 102 (2); g2: shot 102 (2).
    expect(s.station).toEqual([0.5, 2, 2, 0, 0]);
  });

  it('an error shot colours both its endpoint stations red', () => {
    expect(scores(fixture(), [warn(1, 101, 'error')]).station).toEqual([3, 3, 0, 0, 0]);
  });

  it('shares the score across a transitive merge group', () => {
    const s = scores(fixture({ merged: [[2, 3, 4]] }), [warn(1, 102, 'error')]);
    expect(s.station).toEqual([0, 3, 3, 3, 3]);
  });

  it('a station with no shots and no flagged merge partner stays 0', () => {
    const data = buildSurveyData({
      sections: [
        { id: 1, shotIds: [101] },
        { id: 3, shotIds: [] },
      ],
      stations: [
        { sectionId: 1, stationIndex: 0 },
        { sectionId: 1, stationIndex: 1 },
        { sectionId: 3, stationIndex: 0 },
      ],
    });
    expect(scores(data, [warn(1, 101, 'error')]).station[2]).toBe(0);
  });

  it('over-anchoring colours every datum station, not just the representative shot', () => {
    const w: ProjectWarning = {
      ...warn(1, 101, 'error', { code: 'multiple_datums_per_merge_component' }),
      datumRefs: [
        { section_id: 1, station_index: 0 },
        { section_id: 2, station_index: 0 },
      ],
    };
    const s = scores(fixture(), [w]);
    expect(s.station[0]).toBe(3);
    expect(s.station[3]).toBe(3);
  });
});

describe('resolveIssueRender', () => {
  const none = [] as const;
  const input = (over: Partial<IssueRenderInput> = {}): IssueRenderInput => ({
    color: true,
    labels: null,
    warnings: [],
    thresholds: T,
    ...over,
  });

  it('is null while neither the colouring nor the labels are on', () => {
    expect(resolveIssueRender(fixture(), null, none)).toBeNull();
    expect(resolveIssueRender(fixture(), input({ color: false }), none)).toBeNull();
  });

  it('returns the same objects while nothing they depend on changes', () => {
    const data = fixture();
    const i = input();
    const a = resolveIssueRender(data, i, none);
    expect(resolveIssueRender(data, { ...i }, none)).toBe(a);
    // New warnings -> new scores.
    expect(resolveIssueRender(data, input({ warnings: [] }), none)!.scores).not.toBe(a!.scores);
  });

  it('a threshold change recomputes; the same values do not', () => {
    const data = fixture({ shots: { 0: { azimuthIn: 90, azimuthOut: 100 } } });
    const warnings: ProjectWarning[] = [];
    const a = resolveIssueRender(data, input({ warnings }), none)!;
    expect(resolveIssueRender(data, input({ warnings, thresholds: { ...T } }), none)).toBe(a);
    const b = resolveIssueRender(data, input({ warnings, thresholds: { ...T, azimuthDiff: 5 } }), none)!;
    expect(b.scores.shotScore[0]).toBe(2);
  });

  it('carries the label input only while the labels are on, reusing the scores', () => {
    const data = fixture();
    const warnings: ProjectWarning[] = [];
    const colourOnly = resolveIssueRender(data, input({ warnings }), none)!;
    expect(colourOnly.labels).toBeNull();
    const format = { precision: undefined, lengthUnit: 'meters' as const, abbrev: { az: 'a', inc: 'i', dep: 'd', len: 'l', loop: 'o' } };
    const withLabels = resolveIssueRender(data, input({ warnings, labels: format }), none)!;
    expect(withLabels.labels?.format).toBe(format);
    expect(withLabels.scores).toBe(colourOnly.scores);
  });
});
