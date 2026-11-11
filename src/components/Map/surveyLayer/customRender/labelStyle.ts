/**
 * Per-kind label STYLE config + per-frame uniform computation (#customRender
 * labels, Phase 2).
 *
 * The single home for the numbers the 5 stock symbol layers bake into their
 * `text-size` / `text-opacity` / `text-halo-width` / `minzoom` / halo-theme-key
 * (see the parity inventory). The label GL layer holds none of this; each frame
 * it calls {@link computeLabelUniforms} with the LIVE zoom + the controller's
 * style inputs (theme halo colors, per-kind visibility toggles, DPR,
 * reference-hide) and gets the full per-kind uniform arrays the shader reads.
 * Recomputing every frame (not on a zoom band only) reproduces MapLibre's
 * CONTINUOUS `interpolate` size/opacity ramps exactly, at negligible cost (five
 * lerps). Decimation is NO LONGER a per-frame uniform: it is folded into the
 * per-label reveal zoom (`labelReveal.ts` / `labelDecimation.ts`) so the shader
 * only compares the live zoom against the baked `a_reveal_zoom`.
 *
 * PURE: no GL, no DOM. Phase 3 adds the other four kinds' toggles/halo wiring on
 * top of this same table - only the controller's `kindEnabled` / `haloColors`
 * inputs grow; the table already carries all five kinds.
 */
import type { MapThemeColors } from '../../../../utils/themeColors';
import { packCssColor, type Rgba8 } from './colorPack';
import { LABEL_KIND, LABEL_KIND_COUNT, type LabelKind } from './labelShaders';

/** A MapLibre-style `interpolate(linear, zoom, ...)` stop list: `[zoom, value]`
 *  pairs in ascending zoom. A single stop == a constant (zoom-independent). */
export type Stops = readonly (readonly [zoom: number, value: number])[];

/** Which theme halo color a kind uses (parity: section names use `textHalo`,
 *  every other kind uses `textHaloAlt`). */
type HaloThemeKey = 'textHalo' | 'textHaloAlt';

/** Per-kind static style: the zoom ramps + fixed weights the stock layers use. */
export interface KindStyle {
  /** `text-size` stops (CSS px). */
  size: Stops;
  /** `text-opacity` stops (0..1). */
  opacity: Stops;
  /** `text-halo-width` (CSS px). */
  haloWidth: number;
  /** Static `minzoom` gate (below this the kind is hidden). */
  minzoom: number;
  /** Theme key for the halo color. */
  halo: HaloThemeKey;
}

/** Comment halo width base - matches SurveyLayer's `COMMENT_HALO_WIDTH_BASE`. */
const COMMENT_HALO_WIDTH = 1.5;

/**
 * Per-kind style table, mirroring the 5 stock symbol layers byte-for-byte (see
 * the parity inventory §1):
 *   - shot label:     size 14→9 / 18→12,  opacity 14→0.7 / 18→1, halo 2,   minzoom 14, alt
 *   - station label:  size 14→9 / 18→11,  opacity 14→0.7 / 18→1, halo 2,   minzoom 14, alt
 *   - section name:   size 12→11 / 18→14, opacity 1 (const),     halo 1.5, minzoom 10, main
 *   - comments:       size 14→8 / 18→10,  opacity 0.85 (const),  halo 1.5, minzoom 14, alt
 *   - errors (shot + closure): size 14→9 / 18→12, opacity 1, halo 2, minzoom 14, alt
 */
export const LABEL_KIND_STYLE: Record<LabelKind, KindStyle> = {
  [LABEL_KIND.SHOT_LABEL]: {
    size: [[14, 9], [18, 12]],
    opacity: [[14, 0.7], [18, 1]],
    haloWidth: 2,
    minzoom: 14,
    halo: 'textHaloAlt',
  },
  [LABEL_KIND.STATION_LABEL]: {
    size: [[14, 9], [18, 11]],
    opacity: [[14, 0.7], [18, 1]],
    haloWidth: 2,
    minzoom: 14,
    halo: 'textHaloAlt',
  },
  [LABEL_KIND.SECTION_NAME]: {
    size: [[12, 11], [18, 14]],
    opacity: [[0, 1]],
    haloWidth: 1.5,
    minzoom: 10,
    halo: 'textHalo',
  },
  [LABEL_KIND.STATION_COMMENT]: {
    size: [[14, 8], [18, 10]],
    opacity: [[0, 0.85]],
    haloWidth: COMMENT_HALO_WIDTH,
    minzoom: 14,
    halo: 'textHaloAlt',
  },
  [LABEL_KIND.SHOT_COMMENT]: {
    size: [[14, 8], [18, 10]],
    opacity: [[0, 0.85]],
    haloWidth: COMMENT_HALO_WIDTH,
    minzoom: 14,
    halo: 'textHaloAlt',
  },
  // Errors labels: the shot-label size, fully opaque (they are coloured by
  // severity, and a faded red reads as a lesser error), the same halo.
  [LABEL_KIND.SHOT_ERROR]: {
    size: [[14, 9], [18, 12]],
    opacity: [[0, 1]],
    haloWidth: 2,
    minzoom: 14,
    halo: 'textHaloAlt',
  },
  [LABEL_KIND.CLOSURE_ERROR]: {
    size: [[14, 9], [18, 12]],
    opacity: [[0, 1]],
    haloWidth: 2,
    minzoom: 14,
    halo: 'textHaloAlt',
  },
};

/**
 * Linearly interpolate a MapLibre `interpolate(linear, zoom, ...)` stop list at
 * `zoom`, clamping below the first / above the last stop (MapLibre's flat-hold
 * outside the stop range). A single stop is a constant.
 */
export function interpolateStops(zoom: number, stops: Stops): number {
  const first = stops[0]!;
  if (stops.length === 1 || zoom <= first[0]) return first[1];
  const last = stops[stops.length - 1]!;
  if (zoom >= last[0]) return last[1];
  for (let i = 1; i < stops.length; i++) {
    const [z1, v1] = stops[i]!;
    if (zoom <= z1) {
      const [z0, v0] = stops[i - 1]!;
      const t = z1 === z0 ? 0 : (zoom - z0) / (z1 - z0);
      return v0 + (v1 - v0) * t;
    }
  }
  return last[1];
}

/** Resolve the per-kind halo colors from the theme (section names → `textHalo`,
 *  the rest → `textHaloAlt`), packed to RGBA8. The controller passes the result
 *  into {@link LabelStyleInputs.haloColors}; recomputed on theme change only. */
export function resolveHaloColors(theme: MapThemeColors): Rgba8[] {
  const out: Rgba8[] = new Array(LABEL_KIND_COUNT);
  for (let k = 0; k < LABEL_KIND_COUNT; k++) {
    out[k] = packCssColor(theme[LABEL_KIND_STYLE[k as LabelKind].halo]);
  }
  return out;
}

/** The controller-owned, NON-zoom inputs the layer combines with live zoom each
 *  frame: DPR (device-px scaling), per-kind on/off (setShow* toggles), per-kind
 *  halo colors (theme), and the reference-shots-hidden flag. */
export interface LabelStyleInputs {
  dpr: number;
  /** Per-kind visibility (toggle state); combined with the static minzoom gate. */
  kindEnabled: readonly boolean[];
  /** Per-kind halo color RGBA8 (from {@link resolveHaloColors}). */
  haloColors: readonly Rgba8[];
  /** setShowReferenceShots(false) → collapse Reference-shot labels. */
  hideReference: boolean;
  /** setShowJumps(false) → collapse Jump-shot labels (#27, independent of the
   *  reference toggle). An all-reference station/section label needs BOTH. */
  hideJump: boolean;
}

/** The full per-kind uniform set the label shader reads (§3 of the design). All
 *  arrays are length {@link LABEL_KIND_COUNT}; `haloColor` is 4 floats per kind. */
export interface LabelUniforms {
  /** `u_text_size_px[5]` - DEVICE px (CSS size × DPR). */
  textSizePx: Float32Array;
  /** `u_opacity[5]` - 0..1. */
  opacity: Float32Array;
  /** `u_kind_visible[5]` - toggle AND (zoom ≥ minzoom). */
  kindVisible: Float32Array;
  /** `u_halo_color[5]` - RGBA 0..1, 4 floats per kind. */
  haloColor: Float32Array;
  /** `u_halo_width_px[5]` - DEVICE px (CSS halo × DPR, so the halo/size ratio the
   *  fragment uses is DPR-invariant → a constant CSS-px halo). */
  haloWidthPx: Float32Array;
  /** `u_hide_reference` scalar (0/1). */
  hideReference: number;
  hideJump: number;
}

/**
 * Compute every per-kind uniform for the current `zoom` + style inputs. Called
 * once per frame by the GL layer; five short loops, no allocation beyond the
 * fixed-size output arrays.
 */
export function computeLabelUniforms(zoom: number, inputs: LabelStyleInputs): LabelUniforms {
  const n = LABEL_KIND_COUNT;
  const textSizePx = new Float32Array(n);
  const opacity = new Float32Array(n);
  const kindVisible = new Float32Array(n);
  const haloColor = new Float32Array(n * 4);
  const haloWidthPx = new Float32Array(n);
  for (let k = 0; k < n; k++) {
    const cfg = LABEL_KIND_STYLE[k as LabelKind];
    textSizePx[k] = interpolateStops(zoom, cfg.size) * inputs.dpr;
    opacity[k] = interpolateStops(zoom, cfg.opacity);
    kindVisible[k] = inputs.kindEnabled[k] && zoom >= cfg.minzoom ? 1 : 0;
    haloWidthPx[k] = cfg.haloWidth * inputs.dpr;
    const c = inputs.haloColors[k] ?? ([0, 0, 0, 255] as Rgba8);
    haloColor[k * 4] = c[0] / 255;
    haloColor[k * 4 + 1] = c[1] / 255;
    haloColor[k * 4 + 2] = c[2] / 255;
    haloColor[k * 4 + 3] = c[3] / 255;
  }
  return {
    textSizePx,
    opacity,
    kindVisible,
    haloColor,
    haloWidthPx,
    hideReference: inputs.hideReference ? 1 : 0,
    hideJump: inputs.hideJump ? 1 : 0,
  };
}
