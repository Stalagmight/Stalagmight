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

/** Below this recorded inclination (degrees) the length delta is withheld.
 *  At 15° a 1° inclination error moves the implied length of a 10 m shot by
 *  about 0.65 m; at 5° by about 7 m. */
export const LENGTH_DELTA_MIN_INCLINATION_DEG = 15;

/** Score at a check's warning threshold (yellow). */
export const SCORE_WARNING = 1;
/** Highest score: an error, or 3x a warning threshold (red). */
export const SCORE_MAX = 3;

/** The project thresholds a metric is scored against, in degrees. */
export interface ShotQualityThresholds {
  azimuthDiff: number;
  pitchDiff: number;
  inclinationDepthDiff: number;
}

export interface ShotQualityMetrics {
  azDelta: Float32Array;
  pitchDelta: Float32Array;
  inclDepthDelta: Float32Array;
  lengthDelta: Float32Array;
}

/** One bit per metric, for the per-shot "ignored" mask. */
export const METRIC_AZ = 1;
export const METRIC_PITCH = 2;
export const METRIC_INCL_DEPTH = 4;
export const METRIC_LENGTH = 8;

/** Ignoring one of these warnings drops the metrics it is about from that
 *  shot's colour and label. The length delta restates the inclination-vs-depth
 *  mismatch, so it goes with it. */
export const WARNING_CODE_METRICS: Readonly<Record<string, number>> = {
  AZIMUTH_DEVIATION: METRIC_AZ,
  PITCH_DEVIATION: METRIC_PITCH,
  INCLINATION_DEPTH_MISMATCH: METRIC_INCL_DEPTH | METRIC_LENGTH,
};

/**
 * Score one metric against its threshold: 0 for agreement, 1 at the warning
 * threshold, capped at 3. `score > 1` exactly when the validator warns
 * (`|v| > threshold`). A threshold of 0 or less means any disagreement is
 * beyond it, so a non-zero delta scores the maximum. A non-computable (NaN)
 * value scores 0.
 */
export function metricScore(value: number, threshold: number): number {
  const a = Math.abs(value);
  if (!(a > 0)) return 0;
  if (!(threshold > 0)) return SCORE_MAX;
  return Math.min(a / threshold, SCORE_MAX);
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
    inclDepthDelta[i] = rec - impliedPitchDeg(length, depthIn, depthOut);
    const dz = depthOut - depthIn;
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

/** The highest score among a shot's metrics that are not in `ignored`. */
export function shotMetricScore(
  m: ShotQualityMetrics,
  i: number,
  t: ShotQualityThresholds,
  ignored: number,
): number {
  let s = 0;
  if (!(ignored & METRIC_AZ)) s = Math.max(s, metricScore(m.azDelta[i]!, t.azimuthDiff));
  if (!(ignored & METRIC_PITCH)) s = Math.max(s, metricScore(m.pitchDelta[i]!, t.pitchDiff));
  if (!(ignored & METRIC_INCL_DEPTH)) s = Math.max(s, metricScore(m.inclDepthDelta[i]!, t.inclinationDepthDiff));
  return s;
}
