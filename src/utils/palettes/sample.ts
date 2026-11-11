/**
 * Palette sampling: palette position (0..1) -> colour.
 *
 * A {@link ResolvedPalette} is a palette bound to its direction and, for the
 * "Theme" palette, to the active theme's endpoints for one kind. Every consumer
 * (map resolvers, labels, legends, the distance overlay, export) samples
 * through it, so they cannot drift apart.
 *
 * The Theme palette reuses the exact arithmetic the app has always used, so the
 * default scale is byte-identical to the pre-palette colours (#380):
 *   - depth: getDepthColor's shortest-hue HSL walk (`hslRampAt`, rounding);
 *   - distance: MapLibre's floor-packed RGB lerp (`packRampColor`'s maths),
 *     with its exact-endpoint short-circuits.
 * Generated and curated palettes interpolate linearly in RGB between stops and
 * round, or pick a hard band for discrete palettes.
 */
import { SEVERITY_GREEN, SEVERITY_RED, SEVERITY_YELLOW, hexToHSL, hslRampAt, interpolateColor } from '../colors';
import { DISTANCE_FAR, DISTANCE_NEAR } from '../distanceGradient';
import type { Palette, PaletteKind } from './types';

export type Rgb = readonly [r: number, g: number, b: number];

export interface ResolvedPalette {
  /** Equal keys mean byte-identical colours for every `t`. */
  readonly key: string;
  readonly isTheme: boolean;
  /** Band count for a discrete palette (drawn with hard stops), else null. */
  readonly bands: number | null;
  /** `#rrggbb` at palette position `t` (0..1), or null when unusable
   *  (a theme endpoint that is not a hex colour). */
  hex(t: number): string | null;
  /** Integer channels at `t`, or null when unusable. */
  rgb(t: number): Rgb | null;
  /** The colour for a degenerate scale (`min === max`): the palette start. */
  flatHex(): string | null;
}

/** The theme colours the Theme palette depends on. */
export interface ThemeRampEndpoints {
  readonly depthShallow: string;
  readonly depthDeep: string;
}

const HEX2: readonly string[] = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, '0'));

export function rgbHex(r: number, g: number, b: number): string {
  return '#' + HEX2[r]! + HEX2[g]! + HEX2[b]!;
}

function parseHexRgb(hex: string): Rgb | null {
  const m = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  if (!m) return null;
  return [parseInt(m[1]!, 16), parseInt(m[2]!, 16), parseInt(m[3]!, 16)];
}

function byte(x: number): number {
  return x <= 0 ? 0 : x >= 255 ? 255 : Math.round(x);
}

function stopsRgb(data: readonly number[], t: number): Rgb {
  const n = data.length / 4;
  if (n === 1 || t <= data[0]!) return [data[1]!, data[2]!, data[3]!];
  const last = (n - 1) * 4;
  if (t >= data[last]!) return [data[last + 1]!, data[last + 2]!, data[last + 3]!];
  // Binary search for the stop pair bracketing t.
  let lo = 0;
  let hi = n - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (data[mid * 4]! <= t) lo = mid;
    else hi = mid;
  }
  const a = lo * 4;
  const b = hi * 4;
  const f = (t - data[a]!) / (data[b]! - data[a]!);
  return [
    byte(data[a + 1]! + (data[b + 1]! - data[a + 1]!) * f),
    byte(data[a + 2]! + (data[b + 2]! - data[a + 2]!) * f),
    byte(data[a + 3]! + (data[b + 3]! - data[a + 3]!) * f),
  ];
}

function discreteRgb(data: readonly number[], t: number): Rgb {
  const n = data.length / 3;
  const k = t <= 0 ? 0 : Math.min(n - 1, Math.floor(t * n));
  return [data[k * 3]!, data[k * 3 + 1]!, data[k * 3 + 2]!];
}

/** MapLibre's floor-packed channel lerp: `floor(255 * (a01 + t*(b01-a01)))`,
 *  operation for operation as `overlayColor.ts`'s `lerpChannelFloor`. */
function lerpFloor(aByte: number, bByte: number, t: number): number {
  const a01 = aByte / 255;
  const b01 = bByte / 255;
  const x = 255 * (a01 + t * (b01 - a01));
  if (Number.isNaN(x) || x < 0) return 0;
  if (x > 255) return 255;
  return Math.floor(x);
}

function themeDepth(endpoints: ThemeRampEndpoints, reversed: boolean): ResolvedPalette {
  const start = reversed ? endpoints.depthDeep : endpoints.depthShallow;
  const end = reversed ? endpoints.depthShallow : endpoints.depthDeep;
  const a = hexToHSL(start);
  const b = hexToHSL(end);
  const hex = (t: number): string | null => (a && b ? hslRampAt(t, a, b) : null);
  return {
    key: `theme-depth|${start}|${end}`,
    isTheme: true,
    bands: null,
    hex,
    rgb: (t) => {
      const h = hex(t);
      return h ? parseHexRgb(h) : null;
    },
    // getDepthColor returns the shallow colour string itself for a flat range.
    flatHex: () => (a ? start : null),
  };
}

function themeDistance(reversed: boolean): ResolvedPalette {
  const near = parseHexRgb(reversed ? DISTANCE_FAR : DISTANCE_NEAR)!;
  const far = parseHexRgb(reversed ? DISTANCE_NEAR : DISTANCE_FAR)!;
  const rgb = (t: number): Rgb => {
    // The stock interpolate's endpoint short-circuits, so the ends never drift.
    if (t <= 0) return near;
    if (t >= 1) return far;
    return [lerpFloor(near[0], far[0], t), lerpFloor(near[1], far[1], t), lerpFloor(near[2], far[2], t)];
  };
  return {
    key: `theme-distance|${reversed ? 1 : 0}`,
    isTheme: true,
    bands: null,
    hex: (t) => {
      const c = rgb(t);
      return rgbHex(c[0], c[1], c[2]);
    },
    rgb,
    flatHex: () => rgbHex(near[0], near[1], near[2]),
  };
}

/**
 * The Survey Issues default: green -> yellow (the middle, the warn colour) ->
 * red, interpolated in RGB like the original ramp. The map's own default path
 * (`issueColor.ts`) quantises a score to hundredths first; position `t` maps to
 * score `2t` below the middle and `1 + 4(t - 0.5)` above, so the two agree on
 * every quantised score.
 */
function themeIssues(reversed: boolean): ResolvedPalette {
  const hex = (t0: number): string => {
    const t = reversed ? 1 - t0 : t0;
    return t <= 0.5
      ? interpolateColor(SEVERITY_GREEN, SEVERITY_YELLOW, t * 2)
      : interpolateColor(SEVERITY_YELLOW, SEVERITY_RED, (t - 0.5) * 2);
  };
  return {
    key: `theme-issues|${reversed ? 1 : 0}`,
    isTheme: true,
    bands: null,
    hex,
    rgb: (t) => parseHexRgb(hex(t)),
    flatHex: () => hex(0),
  };
}

function dataPalette(palette: Palette, reversed: boolean): ResolvedPalette {
  const discrete = palette.kind === 'discrete';
  const sample = discrete ? discreteRgb : stopsRgb;
  const rgb = (t: number): Rgb => sample(palette.data, reversed ? 1 - t : t);
  return {
    key: `${palette.id}|${reversed ? 1 : 0}`,
    isTheme: false,
    bands: discrete ? palette.data.length / 3 : null,
    hex: (t) => {
      const c = rgb(t);
      return rgbHex(c[0], c[1], c[2]);
    },
    rgb,
    flatHex: () => {
      const c = rgb(0);
      return rgbHex(c[0], c[1], c[2]);
    },
  };
}

/** Bind a palette to a direction and (for Theme) to the theme and kind. */
export function resolvePalette(
  palette: Palette,
  reversed: boolean,
  kind: PaletteKind,
  endpoints: ThemeRampEndpoints,
): ResolvedPalette {
  if (palette.kind === 'theme') {
    if (kind === 'issues') return themeIssues(reversed);
    return kind === 'depth' ? themeDepth(endpoints, reversed) : themeDistance(reversed);
  }
  return dataPalette(palette, reversed);
}

/**
 * The palette baked into `n` packed RGBA8 entries (`t = i / (n - 1)`), for the
 * live-preview recolour, which looks colours up instead of interpolating.
 * Unusable entries are opaque magenta, the colour-pack fallback.
 */
export function bakeLut(resolved: ResolvedPalette, n = 1024): Uint8Array {
  const lut = new Uint8Array(n * 4);
  for (let i = 0; i < n; i++) {
    const c = resolved.rgb(i / (n - 1)) ?? ([255, 0, 255] as const);
    lut[i * 4] = c[0];
    lut[i * 4 + 1] = c[1];
    lut[i * 4 + 2] = c[2];
    lut[i * 4 + 3] = 255;
  }
  return lut;
}

/** A CSS `linear-gradient` of the palette, for swatches and the Levels bar.
 *  Discrete palettes get hard bands; continuous ones 17 samples. */
export function cssGradient(resolved: ResolvedPalette, direction = 'to right'): string {
  const parts: string[] = [];
  if (resolved.bands !== null) {
    const n = resolved.bands;
    for (let k = 0; k < n; k++) {
      const c = resolved.hex((k + 0.5) / n) ?? '#ff00ff';
      parts.push(`${c} ${((k / n) * 100).toFixed(2)}%`, `${c} ${(((k + 1) / n) * 100).toFixed(2)}%`);
    }
  } else {
    const steps = 16;
    for (let i = 0; i <= steps; i++) {
      parts.push(`${resolved.hex(i / steps) ?? '#ff00ff'} ${((i / steps) * 100).toFixed(2)}%`);
    }
  }
  return `linear-gradient(${direction}, ${parts.join(', ')})`;
}
