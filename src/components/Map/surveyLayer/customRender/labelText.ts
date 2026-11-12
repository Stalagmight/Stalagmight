/**
 * The single source of truth for label TEXT + label COLOR (#customRender labels,
 * Phase 1).
 *
 * `features.ts` (the stock vector-tile feature builder) and the custom label
 * tessellator both need the exact same strings ("312° 4.2m 1.8m", a station
 * index, a section name, a comment) and the exact same per-label color (depth
 * ramp vs `labelColor`, section color, comment palette). Reimplementing either
 * on the custom path would risk a subtle A/B mismatch (precision, the
 * depth-vs-flat fallback), so both paths call THESE helpers - parity by
 * construction, the same pattern `colorResolvers.ts` uses for shot/station
 * fills. `features.ts` imports this module (never the reverse), so there is no
 * import cycle.
 *
 * PURE: no DOM, no GL. Depends only on the i18n number formatter (locale-aware,
 * exactly as the stock builder formats), the depth ramp, and the station-name
 * display sanitizer.
 */
import type { DecimalPrecision } from '../../../../types/config';
import type { MapThemeColors } from '../../../../utils/themeColors';
import { formatFixed } from '../../../../i18n/format';
import { getDepthColor } from '../../../../utils/colors';
import { displayStationName } from '../../../../utils/formatters';
import { scaleHex, type ColorScaleInput } from '../../../../utils/palettes';

/**
 * Shot-label text: azimuth (°), length (m), depth (m), each at its configured
 * decimal precision. Byte-for-byte the string `features.ts` builds for the
 * `survey-shot-labels` layer. Defaults (azimuth 0, length 1, depth 1) match the
 * stock builder. `depth` is the shot's MIDPOINT depth (`(depth_in + depth_out) /
 * 2`), computed by the caller.
 */
export function formatShotLabel(
  azimuthIn: number,
  length: number,
  depth: number,
  precision?: DecimalPrecision,
): string {
  return `${formatFixed(azimuthIn, undefined, precision?.azimuth ?? 0)}° ${formatFixed(length, undefined, precision?.length ?? 1)}m ${formatFixed(depth, undefined, precision?.depth ?? 1)}m`;
}

/** Station-label text: the station's index, as the stock builder emits it. */
export function formatStationLabel(stationIndex: number): string {
  return String(stationIndex);
}

/** What a station label says: its ID (the index) or its name. */
export type StationLabelText = 'id' | 'name';

/** The longest station name drawn whole. Longer names are cut to one less and
 *  end in an ellipsis. The image export (`image.rs`) uses the same cap. */
export const STATION_NAME_LABEL_MAX_CHARS = 24;

/**
 * Station-label text in names mode. Names come from the file, so they pass the
 * same sanitizer as every other place a name is shown (no control characters or
 * bidi overrides), then are trimmed and capped by code point - never splitting
 * a surrogate pair. `null` when nothing is left: an unnamed station gets no
 * label rather than falling back to its index.
 */
export function formatStationNameLabel(name: string | null | undefined): string | null {
  const clean = displayStationName(name).trim();
  if (clean === '') return null;
  const chars = Array.from(clean);
  if (chars.length <= STATION_NAME_LABEL_MAX_CHARS) return clean;
  return `${chars.slice(0, STATION_NAME_LABEL_MAX_CHARS - 1).join('').trimEnd()}\u2026`;
}

/** Section-name text. `name` is nullable (a section may be unnamed); the stock
 *  `['get','name']` text-field renders empty for null, so this returns `''`. */
export function formatSectionLabel(name: string | null): string {
  return name ?? '';
}

/** Comment text (shot / station). Identity today, but funnelled through here so
 *  any future normalization (trimming, unit suffixing) stays shared. */
export function formatCommentLabel(comment: string): string {
  return comment;
}

/**
 * Inputs that decide a label's color: the depth-gradient toggle + ramp endpoints
 * + range, and the flat fallback `labelColor`. A small, self-contained context
 * (NOT `features.ts`'s `DisplayColorContext`, to avoid an import cycle) built
 * once per build from the theme colors + depth range.
 */
export interface LabelColorContext {
  showDepthGradient: boolean;
  depthShallow: string;
  depthDeep: string;
  depthMin: number;
  depthMax: number;
  /** Flat label color (theme `--cc-depth-text`) used when depth mode is off or
   *  the ramp declines the value. */
  labelColor: string;
  /** The user's depth palette + Levels range; null / absent = the theme ramp
   *  over `depthMin..depthMax` through the original getDepthColor call. */
  depthScale?: ColorScaleInput | null;
}

function labelDepthColor(value: number, ctx: LabelColorContext): string | null {
  if (!ctx.showDepthGradient) return null;
  if (ctx.depthScale) return scaleHex(value, ctx.depthScale, { min: ctx.depthMin, max: ctx.depthMax });
  return getDepthColor(value, ctx.depthShallow, ctx.depthDeep, ctx.depthMin, ctx.depthMax);
}

/**
 * Shot-label color: the depth ramp at the shot's average depth in depth mode,
 * else the flat `labelColor`. Mirrors `features.ts` exactly (`getDepthColor(...)
 * ?? labelColor`, gated on `showDepthGradient`). `avgDepth` is the shot's
 * midpoint depth the caller already computed for the fill.
 */
export function resolveShotLabelColor(avgDepth: number, ctx: LabelColorContext): string {
  return labelDepthColor(avgDepth, ctx) ?? ctx.labelColor;
}

/**
 * Station-label color: the depth ramp at the station's z in depth mode, else the
 * flat `labelColor`. Mirrors `features.ts` (`getDepthColor(z, ...) ?? labelColor`).
 */
export function resolveStationLabelColor(z: number, ctx: LabelColorContext): string {
  return labelDepthColor(z, ctx) ?? ctx.labelColor;
}

/** Section-name color: the section's own color (never depth-ramped), matching
 *  the stock `survey-labels` `['get','color']` = `section.color`. */
export function resolveSectionLabelColor(sectionColor: string): string {
  return sectionColor;
}

/**
 * Build the shared {@link LabelColorContext} from the same three inputs the stock
 * feature builder uses (`showDepthGradient`, the depth range, the theme colors).
 * Both `features.ts` and the custom label controller call this, so shot/station
 * label colors are parity-by-construction (§Risk 4) - a single place resolves the
 * ramp endpoints + range + flat fallback. `depthRange` is taken structurally
 * (`{ min, max }`) so this pure module never imports `features.ts` (which imports
 * it - no cycle).
 */
export function labelColorContextFrom(
  showDepthGradient: boolean,
  depthRange: { min: number; max: number },
  themeColors: Pick<MapThemeColors, 'depthShallow' | 'depthDeep' | 'labelColor'>,
  depthScale: ColorScaleInput | null = null,
): LabelColorContext {
  return {
    depthScale,
    showDepthGradient,
    depthShallow: themeColors.depthShallow,
    depthDeep: themeColors.depthDeep,
    depthMin: depthRange.min,
    depthMax: depthRange.max,
    labelColor: themeColors.labelColor,
  };
}
