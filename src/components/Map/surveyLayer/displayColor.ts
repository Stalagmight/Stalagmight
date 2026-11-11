/**
 * Shared display-color helpers and color-context types.
 *
 * Extracted from `features.ts` so the custom WebGL renderer (which resolves its
 * own per-feature colors) does not have to import from a stock-pipeline module.
 * This is the single home for the comment-color helper and the color-context
 * types consumed by both the stock feature builder and the custom color
 * resolvers.
 */

import { COMMENT_PALETTE, getDepthColor } from '../../../utils/colors';
import { scaleHex, type ColorScaleInput } from '../../../utils/palettes';
import type { MapThemeColors } from '../../../utils/themeColors';
import { type SurveyData, STATION_FLAG_IS_ZERO_DATUM } from '../../../types/survey-data';
import { issueScoreColor } from './issueColor';
import type { ResolvedPalette } from '../../../utils/palettes/sample';

export type DepthRange = { min: number; max: number };

/** FNV-1a 32-bit string hash. Deterministic and fast; used to pick a stable
 *  `COMMENT_PALETTE` slot from a comment's owner identity (#52) - see
 *  `commentColorForKey`. */
function fnv1a32(str: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < str.length; i++) {
    hash ^= str.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/** Deterministic `COMMENT_PALETTE` color for a comment, keyed by its OWNER
 *  identity rather than build order (#52), so the color stays stable when
 *  other comments are added/removed/moved elsewhere in the survey. Exported
 *  for test assertions; callers should use the section-qualified key shapes
 *  below (`${sectionId}:s${shotId}` for shot comments, `${sectionId}:st${
 *  stationIndex}` for station comments) so two sections can't collide on a
 *  shared local index. */
export function commentColorForKey(key: string): string {
  return COMMENT_PALETTE[fnv1a32(key) % COMMENT_PALETTE.length]!;
}

/**
 * Ghost section data for merge preview visualization.
 */
export interface GhostSectionData {
  /** Shot line coordinates (pairs of [from, to] positions in meters) */
  shots: Array<{ from: { x: number; y: number }; to: { x: number; y: number } }>;
  /** Station positions in meters */
  stations: Array<{ x: number; y: number }>;
  /** Section color */
  color: string;
  /** Merge station highlight position (the station being merged) */
  mergeStation?: { x: number; y: number };
}

/**
 * The inputs that decide a shot's / station's display color. Factored out of
 * the builder loops so `SurveyLayer` can resolve the exact same color for a
 * single feature (interact-overlay highlights) that the bulk build produced -
 * parity by construction, not by re-implementation.
 */
export interface DisplayColorContext {
  showDepthGradient: boolean;
  depthRange: DepthRange;
  /** The user's depth palette + Levels range (map legend cog). Null / absent =
   *  the theme ramp over `depthRange`, linear: the pre-palette colours, through
   *  the original `getDepthColor` call. `depthRange` stays the data extent the
   *  auto range falls back to. */
  depthScale?: ColorScaleInput | null;
  themeColors: MapThemeColors;
  /** Only honored when `issueScores` is present (mirrors the builder). */
  showIssueSeverity: boolean;
  /** Graded Survey Issues scores, 0 to 3 per shot / station (`issueScore.ts`). */
  issueScores?: { shotScore: Float32Array; stationScore: Float32Array } | undefined;
  /** The Survey Issues palette (map legend cog). Null / absent = the default
   *  green -> yellow -> red on its original code path. */
  issuePalette?: ResolvedPalette | null;
  /** An analytical overlay (distance gradient / gas reach) active in the CUSTOM
   *  renderer. Null / absent = off. See {@link OverlayColorInput}. The stock
   *  feature builder ignores this - it colors by section/depth/severity and lets
   *  the feature-state paint ramp override on the bulk layers; only the custom
   *  color resolvers (which have no feature-state) read it. */
  overlay?: OverlayColorInput | null;
  /** Move-comment palette tint (#52) active in the CUSTOM renderer. When true, a
   *  shot / station that OWNS a comment resolves to its deterministic
   *  `commentColorForKey` color (everything else keeps its base color), mirroring
   *  the stock `coalesce(['get','commentColor'], base)` paint the move_comment
   *  tool applies. The stock feature builder ignores this (it writes a
   *  `commentColor` PROPERTY the paint reads); only the custom color resolvers
   *  (which have no per-feature properties) recompute the same color at build
   *  time. Yields to an active `overlay` (distance / gas own color outright). */
  commentTint?: boolean;
  /** Plan-mode idle tint (#395 revision): true while Plan mode is active but no
   *  gas origin has been chosen yet - every shot/station paints the gas ramp's
   *  far color (uniform "nothing computed yet" tint) instead of section/depth
   *  color, so the pre-origin map reads coherently with the gas scheme rather
   *  than leaking section colors under it. Yields to an active `overlay` (a real
   *  gas field wins the moment one exists) exactly like `commentTint`. Only the
   *  custom color resolvers read this - the stock feature builder has no
   *  equivalent path since the custom renderer is the sole survey renderer. */
  planIdleTint?: boolean;
}

/**
 * A per-feature analytical-overlay color input for the custom renderer: the
 * distance gradient (#34) or the gas reachable-frontier (#38). Under the stock
 * layers these overlays are a `feature-state` paint ramp on the (hidden) bulk
 * layers; the custom layer has no feature-state, so it resolves the ramp color
 * per feature at build time from the SAME value maps the stock feature-state is
 * fed (keyed by `encodeFeatureId`) and the SAME interpolate scale `max`. The
 * near/far hues and the grey unreachable color are chosen from `mode` +
 * theme by the custom color module, so the resulting bytes match the stock
 * ramp. `-1` (or a missing key) means unreachable and renders grey.
 */
export interface OverlayColorInput {
  mode: 'distance' | 'gasReach';
  /** featureId -> value (shot: avg of its endpoints; -1 = unreachable). */
  shots: ReadonlyMap<number, number>;
  /** featureId -> value (the station's own; -1 = unreachable). */
  stations: ReadonlyMap<number, number>;
  /** Interpolate scale far end (distance: max reachable; gas: turn budget). */
  max: number;
  /** Distance only: the user's palette + Levels range. Null / absent = the
   *  fixed near/far ramp over `0 -> max`. Gas reach ignores it. */
  scale?: ColorScaleInput | null;
}

export function normalizeDepthRange(depthRange: DepthRange): DepthRange {
  const minD = Number.isFinite(depthRange.min) ? depthRange.min : 0;
  const maxD = Number.isFinite(depthRange.max) ? depthRange.max : 50;
  return {
    min: Math.min(minD, maxD),
    max: Math.max(minD, maxD),
  };
}

/** A depth value's ramp colour, or null (the caller falls back to section
 *  colour). The user's palette when one is set, else the original theme ramp. */
export function depthColorFor(value: number, ctx: DisplayColorContext): string | null {
  if (ctx.depthScale) return scaleHex(value, ctx.depthScale, ctx.depthRange);
  return getDepthColor(
    value,
    ctx.themeColors.depthShallow,
    ctx.themeColors.depthDeep,
    ctx.depthRange.min,
    ctx.depthRange.max,
  );
}

/** The shot's base display color: issue score > depth ramp > section color. */
export function resolveShotDisplayColor(
  data: SurveyData,
  sectionColor: string,
  shotGlobalIdx: number,
  ctx: DisplayColorContext,
): string {
  if (ctx.showIssueSeverity && ctx.issueScores) {
    return issueScoreColor(ctx.themeColors, ctx.issueScores.shotScore[shotGlobalIdx] ?? 0, ctx.issuePalette);
  }
  if (ctx.showDepthGradient) {
    const avgDepth = (data.shotDepthIn[shotGlobalIdx]! + data.shotDepthOut[shotGlobalIdx]!) / 2;
    return depthColorFor(avgDepth, ctx) ?? sectionColor;
  }
  return sectionColor;
}

/**
 * A shot's per-endpoint display colors `[fromColor, toColor]`, used to drive a
 * GPU-interpolated depth gradient along the shot (the custom-render equivalent
 * of the stock `survey-shot-gradients` layer). In depth mode the from-endpoint
 * takes the shot's `depth_in` color and the to-endpoint its `depth_out` color -
 * the exact endpoints the stock gradient ramps between - so the two endpoints
 * are pixel-identical to the stock gradient (the interior differs only in that
 * the GPU lerps in RGB where the stock layer resamples the HSL ramp per
 * sub-segment). Outside depth mode (severity / section color) both endpoints
 * are the flat `resolveShotDisplayColor`, so a caller can always request
 * endpoints without special-casing the mode.
 */
export function resolveShotEndpointColors(
  data: SurveyData,
  sectionColor: string,
  shotGlobalIdx: number,
  ctx: DisplayColorContext,
): [string, string] {
  const base = resolveShotDisplayColor(data, sectionColor, shotGlobalIdx, ctx);
  // An issue score is one flat color per shot; the gradient is depth-only.
  if (ctx.showIssueSeverity && ctx.issueScores) return [base, base];
  if (ctx.showDepthGradient) {
    const from = depthColorFor(data.shotDepthIn[shotGlobalIdx]!, ctx) ?? base;
    const to = depthColorFor(data.shotDepthOut[shotGlobalIdx]!, ctx) ?? base;
    return [from, to];
  }
  return [base, base];
}

/** The station's fill color: issue score > depth ramp > zero-datum > section color. */
export function resolveStationDisplayColor(
  data: SurveyData,
  sectionColor: string,
  stationGlobalIdx: number,
  ctx: DisplayColorContext,
): string {
  if (ctx.showIssueSeverity && ctx.issueScores) {
    return issueScoreColor(ctx.themeColors, ctx.issueScores.stationScore[stationGlobalIdx] ?? 0, ctx.issuePalette);
  }
  const isZeroDatum = (data.stationFlags[stationGlobalIdx]! & STATION_FLAG_IS_ZERO_DATUM) !== 0;
  if (ctx.showDepthGradient) {
    const z = data.stationCoords[stationGlobalIdx * 3 + 2]!;
    return depthColorFor(z, ctx) ?? sectionColor;
  }
  return isZeroDatum ? ctx.themeColors.zeroDatum : sectionColor;
}

/** The station's stroke color (zero-datum ring in depth mode, else theme stroke). */
export function resolveStationStrokeColor(isZeroDatum: boolean, ctx: DisplayColorContext): string {
  return ctx.showDepthGradient && isZeroDatum
    ? ctx.themeColors.zeroDatum
    : ctx.themeColors.strokeColor;
}
