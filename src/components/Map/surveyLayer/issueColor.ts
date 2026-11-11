/**
 * The Survey Issues colour ramp: the palette's start (score 0, the readings
 * agree) to its middle (score 1, a warn level) to its end (score 3: an error,
 * or the Max level). The default palette is the original green -> yellow ->
 * red. Shared by the shot / station fills, the Errors labels and, while Survey
 * Issues paints the map, the loop-closure lines.
 *
 * The score is quantised to hundredths, so a ramp has 301 distinct colours.
 * The custom renderer's packed-colour cache and the hover / selection overlay
 * then see the same few strings, and an overlay highlight matches its bulk
 * fill byte for byte.
 */
import { getErrorColor, hexToRgb, interpolateColor } from '../../../utils/colors';
import type { MapThemeColors } from '../../../utils/themeColors';
import type { ResolvedPalette } from '../../../utils/palettes/sample';
import {
  levelScore,
  METRIC_CLOSURE,
  SCORE_MAX,
  SCORE_WARNING,
  scoreToPosition,
  type IssueLevels,
  type IssueScale,
} from './issueScale';

const STEPS_PER_UNIT = 100;
const MAX_STEP = SCORE_MAX * STEPS_PER_UNIT;
/** The default ramp last built. Its three colours are fixed across themes, so
 *  this is a one-entry cache that, unlike a keyed map, allocates nothing per
 *  call - `issueScoreColor` runs once per shot and station on every build. */
let lastDefault: { clear: string; warning: string; error: string; ramp: readonly string[] } | null = null;
/** Ramps of user palettes, by palette key (a handful at most). */
const paletteRamps = new Map<string, readonly string[]>();

function defaultRamp(clear: string, warning: string, error: string): readonly string[] {
  if (lastDefault && lastDefault.clear === clear && lastDefault.warning === warning && lastDefault.error === error) {
    return lastDefault.ramp;
  }
  const ramp: string[] = [];
  const warnStep = SCORE_WARNING * STEPS_PER_UNIT;
  for (let q = 0; q <= MAX_STEP; q++) {
    ramp.push(
      q <= warnStep
        ? interpolateColor(clear, warning, q / warnStep)
        : interpolateColor(warning, error, (q - warnStep) / (MAX_STEP - warnStep)),
    );
  }
  lastDefault = { clear, warning, error, ramp };
  return ramp;
}

function paletteRamp(palette: ResolvedPalette): readonly string[] {
  let ramp = paletteRamps.get(palette.key);
  if (!ramp) {
    const out: string[] = [];
    for (let q = 0; q <= MAX_STEP; q++) out.push(palette.hex(scoreToPosition(q / STEPS_PER_UNIT)) ?? '#808080');
    if (paletteRamps.size > 32) paletteRamps.clear();
    paletteRamps.set(palette.key, out);
    ramp = out;
  }
  return ramp;
}

/**
 * The ramp colour for a score (clamped to [0, 3]; NaN reads as 0). `palette`
 * null is the default green -> yellow -> red, on its original code path.
 */
export function issueScoreColor(theme: MapThemeColors, score: number, palette: ResolvedPalette | null = null): string {
  const ramp = palette
    ? paletteRamp(palette)
    : defaultRamp(theme.severityClear, theme.severityWarning, theme.severityError);
  const s = Number.isFinite(score) ? Math.min(Math.max(score, 0), SCORE_MAX) : 0;
  return ramp[Math.round(s * STEPS_PER_UNIT)]!;
}

/** How much a severity colour is darkened for label TEXT on a light map. */
const LIGHT_MAP_TEXT_DARKEN = 0.45;

/**
 * A severity colour readable as label text on this map. The ramp's yellow and
 * green are light colours: on a light map (whose label halo is light too) they
 * all but vanish, so there the text is darkened toward black. On a dark map the
 * colour is returned unchanged. A background that is not a hex colour counts
 * as dark (no change).
 */
export function severityTextColor(color: string, theme: MapThemeColors): string {
  const [r, g, b] = hexToRgb(theme.background);
  const luminance = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
  return luminance > 0.5 ? interpolateColor(color, '#000000', LIGHT_MAP_TEXT_DARKEN) : color;
}

/**
 * A loop closure's error percentage on the shot score scale, through the
 * closure's own levels, so a closure label competes with the shot labels on
 * equal terms. The default levels (Min 0%, Warn 1%, Max 5%) give 1% -> 1,
 * 3% -> 2 and 5% or more -> 3: the edges of the closure line's colour bands.
 */
export function closureErrorScore(errorPercentage: number, levels: IssueLevels): number {
  return levelScore(errorPercentage, levels);
}

/**
 * A loop closure's colour: on the Survey Issues scale while it paints the map
 * (`graded`), else the closure line's own colour bands (green < 1%, yellow
 * < 3%, orange < 5%, red).
 */
export function closureColor(
  errorPercentage: number,
  opts: { graded: boolean; levels: IssueLevels; palette: ResolvedPalette | null; theme: MapThemeColors },
): string {
  if (!opts.graded) return getErrorColor(errorPercentage);
  return issueScoreColor(opts.theme, closureErrorScore(errorPercentage, opts.levels), opts.palette);
}

/** A loop-closure LINE's colour for the map's current Survey Issues input:
 *  graded while Survey Issues paints the map and closures are not switched
 *  off, else the line's own colour bands. */
export function closureLineColor(
  errorPercentage: number,
  input: { color: boolean; scale: IssueScale; palette: ResolvedPalette | null } | null,
  theme: MapThemeColors,
): string {
  if (!input?.color || input.scale.hiddenMask & METRIC_CLOSURE) return getErrorColor(errorPercentage);
  return issueScoreColor(theme, closureErrorScore(errorPercentage, input.scale.levels.closure), input.palette);
}
