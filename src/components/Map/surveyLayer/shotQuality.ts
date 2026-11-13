/**
 * Per-shot survey-quality metrics for the graded Survey Issues colouring and
 * the Errors labels.
 *
 * Each metric measures how far two readings of the same shot disagree, the
 * same quantities the Rust validator (`core::validation`) compares against
 * the project's thresholds:
 *
 *   azDelta        |azimuth out - azimuth in|, wrapped into [0, 180]
 *                  (AZIMUTH_DEVIATION, `azimuth_diff_threshold`)
 *   pitchDelta     |inclination out - inclination in|
 *                  (PITCH_DEVIATION, `pitch_diff_threshold`)
 *   inclDepthDelta recorded inclination - the inclination the length and
 *                  depth change imply, signed (+down)
 *                  (INCLINATION_DEPTH_MISMATCH, `inclination_depth_diff_threshold`)
 *   lengthDelta    recorded length - the length the depth change and the
 *                  recorded inclination imply (|dz| / |sin incl|), signed.
 *                  Label-only: it restates the inclination-vs-depth mismatch
 *                  as a distance, so it is not scored on its own. Only on
 *                  shots at least LENGTH_DELTA_MIN_INCLINATION_DEG steep: on a
 *                  shallower shot a 1° clinometer error moves the implied
 *                  length by metres, so the number would mislead.
 *
 * NaN means "not computable" (a leg is missing or out of range, the length is
 * not positive, the shot is too flat for a length, ...). Computed from the
 * recorded shot columns only, so the Raw Data and Closures Computed views share
 * one result: `surveyDataWithAdjustedStations` reuses the shot arrays, and the
 * cache is keyed on them.
 */
import type { SurveyData } from '../../../types/survey-data';
import { lengthFixableFromInclination, lengthFromInclination } from '../../../utils/resolutionPreview';
import { azimuthDelta, impliedPitchDeg, pitchLegInRange, recordedPitch } from '../../../utils/shotGeometry';
import {
  levelScore,
  METRIC_AZ,
  METRIC_INCL_DEPTH,
  METRIC_LENGTH,
  METRIC_PITCH,
  ScoreCombiner,
  type IssueScale,
  type IssueThresholds,
} from './issueScale';

/** Float32-tolerant version of the validator's length-vs-depth-change slack. */
const LENGTH_BELOW_DEPTH_SLACK_M = 1e-3;

/** Below this recorded inclination (degrees) the length delta is withheld.
 *  At 15° a 1° inclination error moves the implied length of a 10 m shot by
 *  about 0.65 m; at 5° by about 7 m. */
export const LENGTH_DELTA_MIN_INCLINATION_DEG = 15;

export { SCORE_MAX, SCORE_WARNING } from './issueScale';
export { METRIC_AZ, METRIC_PITCH, METRIC_INCL_DEPTH, METRIC_LENGTH } from './issueScale';

/** The project thresholds the default levels sit on, in degrees. */
export type ShotQualityThresholds = IssueThresholds;

export interface ShotQualityMetrics {
  azDelta: Float32Array;
  pitchDelta: Float32Array;
  inclDepthDelta: Float32Array;
  lengthDelta: Float32Array;
}

/** Ignoring one of these warnings drops the metrics it is about from that
 *  shot's colour and label. The length delta restates the inclination-vs-depth
 *  mismatch, so it goes with it. */
export const WARNING_CODE_METRICS: Readonly<Record<string, number>> = {
  AZIMUTH_DEVIATION: METRIC_AZ,
  PITCH_DEVIATION: METRIC_PITCH,
  INCLINATION_DEPTH_MISMATCH: METRIC_INCL_DEPTH | METRIC_LENGTH,
};

/**
 * Score one metric against a warning threshold with the default levels (Min 0,
 * Warn at the threshold, Max at 3x): 0 for agreement, 1 at the threshold,
 * capped at 3. `score > 1` when the validator warns (`|v| > threshold`), up to
 * Float32 rounding right at the threshold (the shot columns are Float32, the
 * validator compares f64). A threshold of 0 or less means any disagreement is
 * beyond it, so a non-zero delta scores the maximum. NaN scores 0.
 */
export function metricScore(value: number, threshold: number): number {
  const t = threshold > 0 ? threshold : 0;
  return levelScore(value, { min: 0, warn: t, max: 3 * t });
}

const cache = new WeakMap<Float32Array, ShotQualityMetrics>();

/** The metrics of every shot (indexed by global shot index), cached per shot
 *  column set. */
export function computeShotQualityMetrics(data: SurveyData): ShotQualityMetrics {
  const cached = cache.get(data.shotLength);
  if (cached && cached.azDelta.length === data.shotCount) return cached;

  const n = data.shotCount;
  const azDelta = new Float32Array(n);
  const pitchDelta = new Float32Array(n);
  const inclDepthDelta = new Float32Array(n);
  const lengthDelta = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    azDelta[i] = azimuthDelta(data.shotAzimuthIn[i]!, data.shotAzimuthOut[i]!);

    const pIn = nanToNull(data.shotPitchIn[i]!);
    const pOut = nanToNull(data.shotPitchOut[i]!);
    const legsValid = pitchLegInRange(pIn) && pitchLegInRange(pOut);
    pitchDelta[i] = pIn !== null && pOut !== null && legsValid ? Math.abs(pOut - pIn) : NaN;

    const length = data.shotLength[i]!;
    const depthIn = data.shotDepthIn[i]!;
    const depthOut = data.shotDepthOut[i]!;
    const rec = legsValid ? recordedPitch(pIn, pOut) : null;
    if (rec === null || !(length > 0) || !Number.isFinite(depthIn) || !Number.isFinite(depthOut)) {
      inclDepthDelta[i] = NaN;
      lengthDelta[i] = NaN;
      continue;
    }
    const dz = depthOut - depthIn;
    // A shot shorter than its depth change is the LENGTH_BELOW_DEPTH_CHANGE
    // error, and the validator skips the inclination check there (its implied
    // inclination is clamped to ±90° and means nothing). The margin is looser
    // than Rust's 1e-6 m because these columns are Float32.
    inclDepthDelta[i] =
      length + LENGTH_BELOW_DEPTH_SLACK_M < Math.abs(dz) ? NaN : rec - impliedPitchDeg(length, depthIn, depthOut);
    const steepEnough = Math.abs(rec) >= LENGTH_DELTA_MIN_INCLINATION_DEG;
    const impliedLength =
      steepEnough && lengthFixableFromInclination(dz, rec) ? lengthFromInclination(dz, rec) : null;
    lengthDelta[i] = impliedLength === null ? NaN : length - impliedLength;
  }

  const metrics = { azDelta, pitchDelta, inclDepthDelta, lengthDelta };
  cache.set(data.shotLength, metrics);
  return metrics;
}

function nanToNull(v: number): number | null {
  return Number.isNaN(v) ? null : v;
}

/**
 * A shot's combined score over its errors that are neither `excluded` (ignored
 * on this shot, or switched off) nor uncomputable: the worst, or the average,
 * per the scale. `combiner` is reused across shots to avoid allocating.
 */
export function shotMetricScore(
  m: ShotQualityMetrics,
  i: number,
  scale: IssueScale,
  excluded: number,
  combiner: ScoreCombiner = new ScoreCombiner(scale.combine),
): number {
  combiner.reset();
  const { levels } = scale;
  const add = (bit: number, value: number, id: keyof IssueScale['levels']) => {
    if (!(excluded & bit) && Number.isFinite(value)) combiner.add(levelScore(value, levels[id]));
  };
  add(METRIC_AZ, m.azDelta[i]!, 'azimuth');
  add(METRIC_PITCH, m.pitchDelta[i]!, 'inclination');
  add(METRIC_INCL_DEPTH, m.inclDepthDelta[i]!, 'inclinationDepth');
  add(METRIC_LENGTH, m.lengthDelta[i]!, 'length');
  return combiner.result();
}
