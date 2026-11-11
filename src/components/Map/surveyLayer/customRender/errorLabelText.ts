/**
 * Text and colour of the Errors labels (#customRender labels).
 *
 * A shot gets ONE label holding every disagreement between its readings, so
 * the parts show together or not at all:
 *
 *   Δaz 23.4°  Δinc 4.0°  Δi/d -12.3°  ΔL +0.4m
 *
 *   Δaz   azimuth out vs in
 *   Δinc  inclination out vs in
 *   Δi/d  recorded inclination minus the one the length and depth change imply
 *   ΔL    recorded length minus the one the depth change and inclination imply
 *
 * A part is left out when its value is not computable, rounds to zero at the
 * user's precision, or its warning was ignored. A shot with no parts gets no
 * label. A loop closure gets "Loop 2.1% 0.84m": its stored error percentage
 * and misclosure distance.
 *
 * The prefixes are localized by the caller (`ErrorLabelFormat.abbrev`). Signs
 * are written as ASCII "+" / "-" by hand rather than by `Intl`, which emits
 * U+2212 in some locales. PURE: no DOM, no GL.
 */
import { formatFixed } from '../../../../i18n/format';
import { convertLength } from '../../../../utils/formatStats';
import { getErrorColor } from '../../../../utils/colors';
import type { LengthUnit } from '../../../../types/config';
import type { LoopClosure } from '../../../../types/project';
import type { MapThemeColors } from '../../../../utils/themeColors';
import { issueScoreColor, closureErrorScore } from '../issueColor';
import type { ErrorLabelInput } from '../issueScore';
import { METRIC_AZ, METRIC_INCL_DEPTH, METRIC_LENGTH, METRIC_PITCH } from '../shotQuality';

const DEFAULT_ANGLE_PRECISION = 1;
const DEFAULT_LENGTH_PRECISION = 1;

/** Whether `value` shows as zero at `precision` decimals. */
function roundsToZero(value: number, precision: number): boolean {
  return Math.round(Math.abs(value) * 10 ** precision) === 0;
}

function sign(value: number): string {
  return value > 0 ? '+' : '-';
}

function unitSuffix(unit: LengthUnit): string {
  return unit === 'feet' ? 'ft' : 'm';
}

function angle(prefix: string, value: number, precision: number, signed: boolean): string | null {
  if (!Number.isFinite(value) || roundsToZero(value, precision)) return null;
  const body = formatFixed(Math.abs(value), undefined, precision);
  return `${prefix} ${signed ? sign(value) : ''}${body}°`;
}

function length(prefix: string, meters: number, unit: LengthUnit, precision: number): string | null {
  if (!Number.isFinite(meters)) return null;
  const value = convertLength(meters, unit);
  if (roundsToZero(value, precision)) return null;
  return `${prefix} ${sign(value)}${formatFixed(Math.abs(value), undefined, precision)}${unitSuffix(unit)}`;
}

/** The Errors label text of shot `i` (global index), or null when it has no
 *  part to show. */
export function formatShotErrorLabel(input: ErrorLabelInput, i: number): string | null {
  const { metrics, scores, format } = input;
  const ignored = scores.ignored[i] ?? 0;
  const anglePrecision = format.precision?.azimuth ?? DEFAULT_ANGLE_PRECISION;
  const inclPrecision = format.precision?.inclination ?? DEFAULT_ANGLE_PRECISION;
  const lengthPrecision = format.precision?.length ?? DEFAULT_LENGTH_PRECISION;
  const parts = [
    ignored & METRIC_AZ ? null : angle(format.abbrev.az, metrics.azDelta[i]!, anglePrecision, false),
    ignored & METRIC_PITCH ? null : angle(format.abbrev.inc, metrics.pitchDelta[i]!, inclPrecision, false),
    ignored & METRIC_INCL_DEPTH ? null : angle(format.abbrev.dep, metrics.inclDepthDelta[i]!, inclPrecision, true),
    ignored & METRIC_LENGTH ? null : length(format.abbrev.len, metrics.lengthDelta[i]!, format.lengthUnit, lengthPrecision),
  ].filter((p): p is string => p !== null);
  return parts.length > 0 ? parts.join(' ') : null;
}

/** The colour of shot `i`'s Errors label: the Survey Issues ramp at its label
 *  score, so the worst labels read red at a glance. */
export function shotErrorLabelColor(input: ErrorLabelInput, i: number, theme: MapThemeColors): string {
  return issueScoreColor(theme, input.scores.labelScore[i] ?? 0);
}

/** The Errors label text of a loop closure, or null when its stored metrics
 *  are not finite. */
export function formatClosureErrorLabel(
  closure: LoopClosure,
  prefix: string,
  unit: LengthUnit,
  precision: number = DEFAULT_LENGTH_PRECISION,
): string | null {
  const pct = closure.error_percentage;
  const dist = closure.misclosure_distance;
  if (!Number.isFinite(pct) || !Number.isFinite(dist)) return null;
  const distance = `${formatFixed(convertLength(dist, unit), undefined, precision)}${unitSuffix(unit)}`;
  return `${prefix} ${formatFixed(pct, undefined, 1)}% ${distance}`;
}

/** A closure label's score and colour: the closure line's own colour bands. */
export function closureErrorLabelStyle(closure: LoopClosure): { score: number; color: string } {
  return { score: closureErrorScore(closure.error_percentage), color: getErrorColor(closure.error_percentage) };
}
