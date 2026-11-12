/**
 * Levels mapping: value -> palette position, as Photoshop's Levels dialog
 * maps input tones. Three handles: black point (`min`), white point (`max`)
 * and the grey midpoint, which Photoshop expresses as a gamma.
 *
 *   t  = (clamp(v, min, max) - min) / (max - min)
 *   t' = t ^ (1 / gamma)
 *
 * The midpoint handle is the input whose output is 0.5: `0.5 ^ gamma` of the
 * way from min to max. Moving min or max keeps gamma, so the midpoint handle
 * keeps its relative position, exactly like Photoshop.
 */
import { clamp } from '../clamp';
import type { LevelsScale } from './types';

export const GAMMA_MIN = 0.1;
export const GAMMA_MAX = 9.99;
export const GAMMA_DEFAULT = 1;

/**
 * Palette position (0..1) for a finite value. Callers handle non-finite values
 * and the degenerate `max === min` scale before calling. With gamma 1 this is
 * exactly getDepthColor's `t`, operation for operation, so the default scale
 * reproduces today's colours bit for bit.
 */
export function levelsT(value: number, scale: LevelsScale): number {
  const clamped = clamp(value, scale.min, scale.max);
  const t = (clamped - scale.min) / (scale.max - scale.min);
  return scale.gamma === 1 ? t : Math.pow(t, 1 / scale.gamma);
}

/** Clamp a gamma into Photoshop's range and round it to its 2 decimals. */
export function clampGamma(gamma: number): number {
  if (!Number.isFinite(gamma)) return GAMMA_DEFAULT;
  return Math.round(clamp(gamma, GAMMA_MIN, GAMMA_MAX) * 100) / 100;
}

/** Where the midpoint handle sits, as a fraction of the min..max span. */
export function midFraction(gamma: number): number {
  return Math.pow(0.5, gamma);
}

/** The gamma that puts the midpoint handle at `fraction` of the span. */
export function gammaFromFraction(fraction: number): number {
  const p = clamp(fraction, 1e-6, 1 - 1e-6);
  return clampGamma(Math.log(p) / Math.log(0.5));
}

/** The value under the midpoint handle. */
export function midValue(scale: LevelsScale): number {
  return scale.min + midFraction(scale.gamma) * (scale.max - scale.min);
}

/** The scale every kind starts from: the data's own extent, linear. */
export function autoScale(range: { min: number; max: number }): LevelsScale {
  return { min: range.min, max: range.max, gamma: GAMMA_DEFAULT };
}

export function sameScale(a: LevelsScale | null, b: LevelsScale | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return a.min === b.min && a.max === b.max && a.gamma === b.gamma;
}
