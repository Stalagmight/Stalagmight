/**
 * The Survey Issues colour ramp: green (score 0, the readings agree) to yellow
 * (score 1, a warning threshold) to red (score 3: an error, or three times a
 * warning threshold). Shared by the shot / station fills and the Errors labels.
 *
 * The score is quantised to hundredths, so the ramp has 301 distinct colours.
 * The custom renderer's packed-colour cache and the hover / selection overlay
 * then see the same few strings, and an overlay highlight matches its bulk
 * fill byte for byte.
 */
import { interpolateColor } from '../../../utils/colors';
import type { MapThemeColors } from '../../../utils/themeColors';
import { SCORE_MAX, SCORE_WARNING } from './shotQuality';

const STEPS_PER_UNIT = 100;
const rampCache = new Map<string, readonly string[]>();

function rampFor(clear: string, warning: string, error: string): readonly string[] {
  const key = `${clear}|${warning}|${error}`;
  const hit = rampCache.get(key);
  if (hit) return hit;
  const ramp: string[] = [];
  const warnStep = SCORE_WARNING * STEPS_PER_UNIT;
  const maxStep = SCORE_MAX * STEPS_PER_UNIT;
  for (let q = 0; q <= maxStep; q++) {
    ramp.push(
      q <= warnStep
        ? interpolateColor(clear, warning, q / warnStep)
        : interpolateColor(warning, error, (q - warnStep) / (maxStep - warnStep)),
    );
  }
  rampCache.set(key, ramp);
  return ramp;
}

/** The ramp colour for a score (clamped to [0, 3]; NaN reads as 0). */
export function issueScoreColor(theme: MapThemeColors, score: number): string {
  const ramp = rampFor(theme.severityClear, theme.severityWarning, theme.severityError);
  const s = Number.isFinite(score) ? Math.min(Math.max(score, 0), SCORE_MAX) : 0;
  return ramp[Math.round(s * STEPS_PER_UNIT)]!;
}

/**
 * A loop closure's error percentage on the same 0-3 scale, so a closure label
 * competes with the shot labels on equal terms. The anchors are the band edges
 * the closure line colour already uses (`getErrorColor`): 0% scores 0, 1%
 * scores 1, 3% scores 2 and 5% or more scores 3.
 */
export function closureErrorScore(errorPercentage: number): number {
  if (!(errorPercentage > 0)) return 0;
  if (errorPercentage <= 1) return errorPercentage;
  return Math.min(SCORE_MAX, 1 + (errorPercentage - 1) / 2);
}
