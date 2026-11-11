/**
 * Colour-scale helpers shared by the renderer, labels, legends, the MapLibre
 * distance expression and the export. Imports only `levels` at runtime (types
 * from `sample`), so `utils/distanceGradient.ts` can use it without an import
 * cycle through the theme-distance palette.
 */
import { autoScale, levelsT, sameScale } from './levels';
import type { ResolvedPalette } from './sample';
import type { LevelsScale } from './types';

/**
 * A palette plus its Levels mapping, as the renderer, labels, legends and
 * export consume it. `scale: null` means auto: the caller's data extent,
 * linear - which, with the Theme palette, is exactly the pre-palette ramp.
 */
export interface ColorScaleInput {
  readonly palette: ResolvedPalette;
  readonly scale: LevelsScale | null;
}

/** The effective Levels scale: the explicit one, or the data extent. */
export function effectiveScale(input: ColorScaleInput, auto: { min: number; max: number }): LevelsScale {
  return input.scale ?? autoScale(auto);
}

/**
 * `#rrggbb` for a value through a colour scale, or null (non-finite value or
 * range, or an unusable palette) so the caller falls back the way it always
 * has. A flat range (`min === max`) takes the palette start, as getDepthColor
 * does.
 */
export function scaleHex(value: number, input: ColorScaleInput, auto: { min: number; max: number }): string | null {
  const s = effectiveScale(input, auto);
  if (!Number.isFinite(value) || !Number.isFinite(s.min) || !Number.isFinite(s.max)) return null;
  if (s.max === s.min) return input.palette.flatHex();
  return input.palette.hex(levelsT(value, s));
}

/** Whether two scale inputs colour every value identically. */
export function sameColorScaleInput(
  a: ColorScaleInput | null | undefined,
  b: ColorScaleInput | null | undefined,
): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return a.palette.key === b.palette.key && sameScale(a.scale, b.scale);
}

/**
 * `(value, colour)` stops sampling a scale across its value range, for
 * consumers that interpolate themselves (MapLibre expressions, SVG legends).
 * Values ascend strictly; a flat range yields one stop.
 */
export function scaleStops(
  input: ColorScaleInput,
  auto: { min: number; max: number },
  count = 33,
): Array<readonly [value: number, hex: string]> {
  const s = effectiveScale(input, auto);
  if (!(s.max > s.min)) return [[s.min, input.palette.flatHex() ?? '#808080']];
  const out: Array<readonly [number, string]> = [];
  for (let k = 0; k < count; k++) {
    const v = s.min + ((s.max - s.min) * k) / (count - 1);
    out.push([v, input.palette.hex(levelsT(v, s)) ?? '#808080']);
  }
  return out;
}

/**
 * Stops for a legend bar whose length is the scale's value axis (`min` at 0%,
 * `max` at 100%): colours follow the gamma bend, discrete palettes get hard
 * band edges. A flat range is one solid colour.
 */
export function legendGradientStops(
  input: ColorScaleInput,
  auto: { min: number; max: number },
  samples = 33,
): Array<{ offset: number; color: string }> {
  const s = effectiveScale(input, auto);
  const fallback = '#808080';
  if (!(s.max > s.min)) {
    const c = input.palette.flatHex() ?? fallback;
    return [{ offset: 0, color: c }, { offset: 100, color: c }];
  }
  const out: Array<{ offset: number; color: string }> = [];
  if (input.palette.bands !== null) {
    const n = input.palette.bands;
    for (let k = 0; k < n; k++) {
      const c = input.palette.hex((k + 0.5) / n) ?? fallback;
      out.push({ offset: 100 * Math.pow(k / n, s.gamma), color: c });
      out.push({ offset: 100 * Math.pow((k + 1) / n, s.gamma), color: c });
    }
    return out;
  }
  for (let i = 0; i < samples; i++) {
    const f = i / (samples - 1);
    const v = s.min + (s.max - s.min) * f;
    out.push({ offset: 100 * f, color: input.palette.hex(levelsT(v, s)) ?? fallback });
  }
  return out;
}
