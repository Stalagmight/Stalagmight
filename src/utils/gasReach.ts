import type { ExpressionSpecification } from 'maplibre-gl';
import type { GasReachField } from '../types/ipc.generated';
import type { SurveyData } from '../types/survey-data';
import type { Coordinates, StationRef } from '../types/project';
import { encodeFeatureId } from './ids';

/**
 * Gas reachable-frontier overlay helpers (#38 Phase 1; #395 three-way split).
 *
 * Sibling of {@link ./distanceGradient}: colors shots/stations by the litres of
 * gas it costs to reach them from the origin, using MapLibre `feature-state`
 * (`gasReach`) + a swapped paint `interpolate`. Stations **within the turn
 * budget** carry a colorable value (gold near the origin -> purple at the
 * budget); stations connected to the survey but beyond reach - the gas budget
 * OR the scooter rig's usable range (the backend ANDs both, `gas.rs`
 * `budget_view`'s `in_range`) - clamp to the ramp's FAR color (dimmed, not
 * greyed - see `GAS_DIM_ALPHA`); only topologically UNCONNECTED stations read grey.
 * This three-way split lets a diver tell "past my turn point or scooter range"
 * (dimmed, colored) apart from "not part of this cave section at all" (grey)
 * at a glance. These pure helpers keep the transform unit-testable in
 * isolation.
 */

/** Sentinel `gasReach` for "no path cost solved" - unconnected or a stalled
 *  walk (renders grey). */
export const UNREACHABLE_GAS = -1;

/** Sentinel `gasReach` for "connected to the survey, but beyond the gas/turn
 *  budget" (renders the ramp's far color, dimmed - never grey, never the base
 *  section color; a uniform "just gas reach, but dimmer" clamp). */
export const BEYOND_GAS_RANGE = -2;

/**
 * Reachable-overlay ramp endpoints (near → far). Reuses the distance gradient's
 * gold→purple hues so the two analytical overlays read consistently: gold = little
 * gas spent (near the origin), purple = at the turn budget (your turn point).
 */
export const GAS_NEAR = '#f4d03f'; // gold - little gas spent
export const GAS_FAR = '#7d3c98'; // purple - at the turn budget

/**
 * The set of `${section_id}:${station_index}` keys for stations reachable on the current gas plan.
 * Centralizes the "what counts as reachable" rule + the key format so the reach overlay and the
 * Plan-mode nearest-turnaround pick (`nearestReachableStationRef`) can't drift (#236). `station_refs`
 * and `reachable` are parallel arrays from the backend `GasReachField`.
 */
export function reachableStationKeySet(field: GasReachField): Set<string> {
  const keys = new Set<string>();
  for (let i = 0; i < field.station_refs.length; i++) {
    if (field.reachable[i]) {
      const r = field.station_refs[i]!;
      keys.add(`${r.section_id}:${r.station_index}`);
    }
  }
  return keys;
}

/**
 * Whether the dive reaches stage drop `i` (#576 RV8c-2-D3). A pinned drop the gas - or the scooter
 * range - runs out before is reported by the engine at its pin, PAST the turnaround: its
 * `stage_drop_route_indices` entry is beyond the route's last station (a route of `route.length`
 * legs has stations 0..route.length). The stage is then carried the whole dive, so there is no
 * pressure to leave it at. This only reads the engine's index; nothing here re-derives the gas.
 */
export function stageDropReached(field: GasReachField, i: number): boolean {
  const routeIndex = (field.stage_drop_route_indices ?? [])[i];
  return routeIndex === undefined || routeIndex <= field.route.length;
}

/**
 * Pick which of a shot's two endpoint stations is nearer the CURRENT ORIGIN (#395/#397): every
 * plan-mode point-set click that lands on a shot (not a station) anchors to whichever endpoint
 * is closer to that origin, so the resulting pin reads the same as clicking the nearer station
 * directly. Despite the shape of the name, this does NOT always rank against the entrance - see
 * the two regimes below, which differ in what "origin" means at call time.
 *
 * Two rules, in order:
 * 1. A loaded {@link GasReachField} is engine truth: compare `gas_to_reach` (litres to reach each
 *    endpoint from the current origin) and take the smaller. `null` (unconnected, or the walk
 *    stalled) sorts as infinitely far; if BOTH are null there is no signal, so fall through.
 *    Two regimes fall out of "current origin" here:
 *      - Pre-origin (first click, entrance not yet chosen): the field is null or rooted at the
 *        entrance itself, so `origin == entrance` and this rule ranks by distance-to-entrance.
 *      - Re-armed pick-on-map (`gas-pick-origin`, origin already set once): the loaded field is
 *        still rooted at the OUTGOING origin - the one about to be replaced - not the entrance.
 *        This deliberately ranks the new click against where the diver currently starts, not
 *        where the cave starts; that is the origin a re-pick is meant to move away from.
 * 2. No field yet (the pre-origin click, before an origin exists to solve from) - or both
 *    endpoints tied under rule 1: approximate with straight-line distance to the nearest
 *    zero-datum station, mirroring the engine's own multi-root "entrance" (`zero_datum_roots`,
 *    every zero datum is a root, not just one) without running a graph search client-side. With
 *    NO zero datum on the project the engine's own entrance concept is undefined (a project with
 *    no zero datum resolves an empty root set, so nothing is reachable from "the entrance"
 *    either) - in that unreachable edge case this deterministically keeps `a`, the same endpoint
 *    the old lower-station-index rule always picked, rather than inventing a coordinate origin.
 */
export function nearestOriginShotEndpoint(
  a: StationRef,
  b: StationRef,
  reachField: GasReachField | null | undefined,
  activeStations: ReadonlyArray<{
    station_ref: StationRef;
    coordinates: Coordinates;
    is_zero_datum: boolean;
  }>,
): StationRef {
  if (reachField) {
    const byKey = new Map<string, number | null>();
    for (let i = 0; i < reachField.station_refs.length; i++) {
      const r = reachField.station_refs[i]!;
      byKey.set(`${r.section_id}:${r.station_index}`, reachField.gas_to_reach[i] ?? null);
    }
    const aGas = byKey.get(`${a.section_id}:${a.station_index}`) ?? null;
    const bGas = byKey.get(`${b.section_id}:${b.station_index}`) ?? null;
    if (aGas != null || bGas != null) {
      const aCost = aGas ?? Infinity;
      const bCost = bGas ?? Infinity;
      return aCost <= bCost ? a : b;
    }
  }

  const zeroDatums = activeStations.filter((s) => s.is_zero_datum);
  if (zeroDatums.length === 0) return a;

  const coordsOf = new Map<string, Coordinates>();
  for (const s of activeStations) {
    coordsOf.set(`${s.station_ref.section_id}:${s.station_ref.station_index}`, s.coordinates);
  }
  const nearestDatumDistSq = (ref: StationRef): number => {
    const c = coordsOf.get(`${ref.section_id}:${ref.station_index}`);
    if (!c) return Infinity;
    let best = Infinity;
    for (const zd of zeroDatums) {
      const dx = zd.coordinates.x - c.x;
      const dy = zd.coordinates.y - c.y;
      const dz = zd.coordinates.z - c.z;
      const d = dx * dx + dy * dy + dz * dz;
      if (d < best) best = d;
    }
    return best;
  };
  return nearestDatumDistSq(a) <= nearestDatumDistSq(b) ? a : b;
}

export interface GasReachFeatureState {
  /** featureId → litres-to-reach (avg of the shot's two endpoints), or one of
   *  {@link UNREACHABLE_GAS} / {@link BEYOND_GAS_RANGE} - the "worse" class wins
   *  when the shot's two endpoints disagree (unconnected dominates beyond-reach). */
  shots: Map<number, number>;
  /** featureId → litres-to-reach for the station, or {@link UNREACHABLE_GAS} /
   *  {@link BEYOND_GAS_RANGE}. */
  stations: Map<number, number>;
  /** The turn budget (litres) - the far (purple) end of the interpolate scale. */
  max: number;
}

/** Classify one station's backend fact into the carried `gasReach` value:
 *  `gas_to_reach === null` means no path cost was solved at all - either the
 *  station is topologically unconnected, or the walk STALLED before reaching
 *  it (e.g. zero swim speed/RMV; backend `gas.rs` sets `reach_gas` to
 *  infinity, which serializes as null); a finite cost with `reachable ===
 *  false` means connected but past the turn/gas budget OR the scooter rig's
 *  usable range; otherwise the litres-to-reach are the value. */
function classifyStation(gasToReach: number | null | undefined, reachable: boolean): number {
  if (gasToReach == null) return UNREACHABLE_GAS;
  return reachable ? gasToReach : BEYOND_GAS_RANGE;
}

/** The "worse" of two station classes for a shot spanning them: unconnected
 *  dominates beyond-reach, which dominates any in-reach litre value. */
function worseOf(a: number, b: number): number {
  if (a === UNREACHABLE_GAS || b === UNREACHABLE_GAS) return UNREACHABLE_GAS;
  if (a === BEYOND_GAS_RANGE || b === BEYOND_GAS_RANGE) return BEYOND_GAS_RANGE;
  return (a + b) / 2;
}

/**
 * Turn a backend `GasReachField` into per-feature `gasReach` values. The scale max
 * is the **turn budget** (`budget_litres`), not the deepest reachable cost, so the
 * purple end always means "at your turn pressure" regardless of how much of the
 * cave is in range - spare budget reads as spare color, never a false "at-limit".
 *
 * Reads the struct-of-arrays `SurveyData` (section metadata + `shotIds`) rather
 * than `Section[].shots[]`, for the same reason as `buildDistanceFeatureState`:
 * enabling the overlay must not force-materialize every lazy `Shot` at load.
 */
export function buildGasReachFeatureState(
  data: Pick<SurveyData, 'sections' | 'shotIds'>,
  field: GasReachField,
): GasReachFeatureState {
  // station ref → classified value (litres, UNREACHABLE_GAS, or BEYOND_GAS_RANGE).
  const byRef = new Map<string, number>();
  for (let i = 0; i < field.station_refs.length; i++) {
    const r = field.station_refs[i]!;
    byRef.set(
      `${r.section_id}:${r.station_index}`,
      classifyStation(field.gas_to_reach[i], field.reachable[i] ?? false),
    );
  }

  const stations = new Map<number, number>();
  const shots = new Map<number, number>();

  for (const section of data.sections) {
    const n = section.shotCount;
    // Stations: index 0..=n
    for (let idx = 0; idx <= n; idx++) {
      const v = byRef.get(`${section.id}:${idx}`);
      stations.set(encodeFeatureId(section.id, idx), v ?? UNREACHABLE_GAS);
    }
    // Shots: connect station i → i+1; key by the shot's stable id (from SoA).
    // Conservative rule: if either endpoint is not in-reach, the shot takes the
    // worse of the two classes (unconnected dominates beyond-reach).
    for (let i = 0; i < n; i++) {
      const shotId = data.shotIds[section.shotOffset + i]!;
      const from = byRef.get(`${section.id}:${i}`) ?? UNREACHABLE_GAS;
      const to = byRef.get(`${section.id}:${i + 1}`) ?? UNREACHABLE_GAS;
      shots.set(encodeFeatureId(section.id, shotId), worseOf(from, to));
    }
  }

  return { shots, stations, max: field.budget_litres };
}

/**
 * The featureIds whose value is {@link BEYOND_GAS_RANGE} in a `shots` / `stations`
 * map from {@link GasReachFeatureState} - the custom renderer's gas-dim set (#395).
 * `undefined` (overlay off, or no state yet) yields an empty set.
 */
export function gasBeyondReachFeatureIds(map: Map<number, number> | undefined): ReadonlySet<number> {
  if (!map) return new Set<number>();
  const ids = new Set<number>();
  for (const [fid, value] of map) {
    if (value === BEYOND_GAS_RANGE) ids.add(fid);
  }
  return ids;
}

/**
 * The gas reachable-frontier color ramp (shared by shots line-color and stations
 * circle-color so the two can't drift). Mirrors `distanceColorExpression` but reads
 * the `gasReach` feature-state key, plus a third case: {@link BEYOND_GAS_RANGE}
 * (connected but past budget) clamps to `far` - the ramp's own far-end color -
 * rather than grey or the feature's base color, matching the custom renderer's
 * per-vertex dim ("just gas reach, but dimmer").
 */
export function gasReachColorExpression(
  near: string,
  far: string,
  max: number,
  grey: string,
  gasInput: ExpressionSpecification = ['feature-state', 'gasReach'] as ExpressionSpecification,
): ExpressionSpecification {
  const safeMax = max > 0 ? max : 1;
  return [
    'let',
    'g',
    ['coalesce', gasInput, UNREACHABLE_GAS],
    [
      'case',
      ['==', ['var', 'g'], BEYOND_GAS_RANGE],
      far,
      ['<', ['var', 'g'], 0],
      grey,
      ['interpolate', ['linear'], ['var', 'g'], 0, near, safeMax, far],
    ],
  ] as ExpressionSpecification;
}
