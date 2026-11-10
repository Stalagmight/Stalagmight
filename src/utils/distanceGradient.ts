import type { ExpressionSpecification } from 'maplibre-gl';
import type { DistanceField } from '../types/project';
import type { SurveyData } from '../types/survey-data';
import { encodeFeatureId } from './ids';
import { effectiveScale, scaleStops, type ColorScaleInput } from './palettes/scale';

/**
 * Distance-gradient overlay helpers (#34).
 *
 * The overlay colors shots/stations by along-cave distance from a chosen origin
 * using MapLibre `feature-state` (`dist`) + a swapped paint `interpolate`. These
 * pure helpers turn a backend `DistanceField` into per-feature `dist` values and
 * build the paint expression, kept here so they're unit-testable in isolation.
 */

/** Sentinel `dist` for unreachable / unknown features (renders grey). */
export const UNREACHABLE_DIST = -1;

/**
 * Dedicated distance-gradient ramp endpoints (near → far). Fixed hex (like the
 * depth ramp, which also forces constants regardless of theme) and deliberately
 * distinct hues from depth so it reads as a different encoding. (#34)
 */
export const DISTANCE_NEAR = '#f4d03f'; // gold - close to origin
export const DISTANCE_FAR = '#7d3c98'; // purple - far from origin

export interface DistanceFeatureState {
  /** featureId → dist (avg of the shot's two endpoints; -1 if either unreachable) */
  shots: Map<number, number>;
  /** featureId → dist (the station's own distance; -1 if unreachable) */
  stations: Map<number, number>;
  /** Largest finite distance, for the interpolate scale. */
  max: number;
}

/** What an analytical-overlay apply effect should do this render. */
export type OverlayApplyAction = 'ramp' | 'off' | 'hold';

/**
 * Decide how an analytical-overlay apply effect (distance #34 / gas-reach #38 -
 * both mirror this shape) should drive the survey layer for the current
 * `(mode, field, geometry)` tuple. Shared by both overlays; lives here as the
 * canonical overlay helper.
 *
 * The per-feature value map is fetched ASYNCHRONOUSLY: changing the origin / gas
 * parameters clears the cached field to `null` (uiStore.setDistanceGradientOrigin
 * / gasPlanStore setters) and a fresh one lands a tick later. A naive
 * `field ? on : off` effect would drive the layer OFF (base colors) for that one
 * tick and back ON - a visible blink of section colors before the new ramp,
 * worst under the custom renderer where OFF is a full buffer rebuild. This guard
 * splits the "off" case in two:
 *   - `'ramp'`: the field is ready → install / refresh the ramp.
 *   - `'off'` : the mode is genuinely off (disabled, superseded by severity, or
 *               no geometry) → restore base colors immediately (deactivation
 *               must NOT be held). ALSO the clear-on-failure path (see `failed`).
 *   - `'hold'`: the mode is on and geometry is present but the field is
 *               transiently absent (a SUCCESSFUL recompute in flight) → keep the
 *               CURRENT colors (the old ramp), no layer call, until the new field
 *               lands.
 *
 * A genuinely EMPTY result (no reachable features) is still a NON-null field
 * object, so the caller passes `hasField = true` and it maps to `'ramp'` (the
 * all-grey ramp) - never `'hold'` on stale colors.
 *
 * CLEAR ON FAILURE (`failed`): a recompute that REJECTED (the IPC threw) leaves
 * no new field, so `hasField` is false. Without a signal that the compute failed
 * (rather than merely being in flight) the effect would `'hold'` the old ramp
 * against the NEW inputs indefinitely. When the caller sets `failed`, the transient
 * `'hold'` collapses to `'off'` so the stale colors are cleared. The SUCCESS path
 * (`failed` stays false) keeps the blink-guard `'hold'` unchanged.
 */
export function overlayApplyAction(
  modeOn: boolean,
  hasField: boolean,
  hasSurvey: boolean,
  failed = false,
): OverlayApplyAction {
  if (!modeOn || !hasSurvey) return 'off';
  if (hasField) return 'ramp';
  return failed ? 'off' : 'hold';
}

/**
 * Build per-feature `dist` values from a `DistanceField`. Station value is its
 * own distance; shot value is the average of its two endpoint stations (so the
 * gradient flows along passage). Unreachable (`null`) or missing endpoints →
 * `UNREACHABLE_DIST`. Keys use the shared `encodeFeatureId` so they line up with
 * the feature ids produced by `surveyLayer/features.ts`.
 *
 * Reads the struct-of-arrays `SurveyData` (section metadata + `shotIds`) rather
 * than `Section[].shots[]`: the overlay only needs each shot's stable id and its
 * section's shot count, so touching the lazy `section.shots` getter here would
 * force-materialize every `Shot` object at load whenever the overlay is enabled
 * (~75k objects / ~1.3 GB for 92.stalx). The SoA path stays typed-array-only.
 */
export function buildDistanceFeatureState(
  data: Pick<SurveyData, 'sections' | 'shotIds'>,
  field: DistanceField,
): DistanceFeatureState {
  // station ref → distance (null = unreachable)
  const byRef = new Map<string, number | null>();
  for (let i = 0; i < field.station_refs.length; i++) {
    const r = field.station_refs[i]!;
    byRef.set(`${r.section_id}:${r.station_index}`, field.distances[i] ?? null);
  }

  const stations = new Map<number, number>();
  const shots = new Map<number, number>();

  for (const section of data.sections) {
    const n = section.shotCount;
    // Stations: index 0..=n
    for (let idx = 0; idx <= n; idx++) {
      const d = byRef.get(`${section.id}:${idx}`);
      stations.set(encodeFeatureId(section.id, idx), d == null ? UNREACHABLE_DIST : d);
    }
    // Shots: connect station i → i+1; key by the shot's stable id (from SoA).
    for (let i = 0; i < n; i++) {
      const shotId = data.shotIds[section.shotOffset + i]!;
      const from = byRef.get(`${section.id}:${i}`);
      const to = byRef.get(`${section.id}:${i + 1}`);
      const value = from == null || to == null ? UNREACHABLE_DIST : (from + to) / 2;
      shots.set(encodeFeatureId(section.id, shotId), value);
    }
  }

  return { shots, stations, max: field.max_reachable };
}

/**
 * MapLibre color expression for the distance gradient: interpolate `near → far`
 * over `0 → max` on the `dist` feature-state; anything with no/negative `dist`
 * (unset or unreachable) renders `grey`. Guards a degenerate `max <= 0` so the
 * interpolate stops stay strictly ascending.
 *
 * `distInput` overrides where `dist` is read from (default: the `dist`
 * feature-state). The interact overlay passes `['get', 'dist']` so its
 * property-carried value runs through the SAME GPU interpolate and renders
 * pixel-identical colors to the bulk layers.
 */
export function distanceColorExpression(
  near: string,
  far: string,
  max: number,
  grey: string,
  distInput: ExpressionSpecification = ['feature-state', 'dist'] as ExpressionSpecification,
  scale: ColorScaleInput | null = null,
): ExpressionSpecification {
  const safeMax = max > 0 ? max : 1;
  const ramp: ExpressionSpecification = scale
    ? scaledRamp(scale, safeMax)
    : (['interpolate', ['linear'], ['var', 'd'], 0, near, safeMax, far] as ExpressionSpecification);
  return [
    'let',
    'd',
    ['coalesce', distInput, UNREACHABLE_DIST],
    ['case', ['<', ['var', 'd'], 0], grey, ramp],
  ] as ExpressionSpecification;
}

/**
 * The user's palette + Levels range as a MapLibre ramp over `d`: a sampled
 * 33-stop `interpolate` (the gamma bend between samples is linearised, which
 * only the hover / selection overlay sees), or a `step` at the exact band
 * edges for a discrete palette. A flat range is the palette's start colour.
 */
function scaledRamp(input: ColorScaleInput, safeMax: number): ExpressionSpecification {
  const s = effectiveScale(input, { min: 0, max: safeMax });
  if (!(s.max > s.min)) return (input.palette.flatHex() ?? '#808080') as unknown as ExpressionSpecification;
  if (input.palette.bands !== null) {
    const n = input.palette.bands;
    const expr: unknown[] = ['step', ['var', 'd'], input.palette.hex(0) ?? '#808080'];
    for (let k = 1; k < n; k++) {
      // Band k starts where t' = k/n, i.e. t = (k/n)^gamma.
      expr.push(s.min + (s.max - s.min) * Math.pow(k / n, s.gamma), input.palette.hex((k + 0.5) / n) ?? '#808080');
    }
    return expr as ExpressionSpecification;
  }
  const expr: unknown[] = ['interpolate', ['linear'], ['var', 'd']];
  for (const [v, hex] of scaleStops(input, { min: 0, max: safeMax })) expr.push(v, hex);
  return expr as ExpressionSpecification;
}
