import type { PathShot, StationRef } from '../types/project';
import { encodeFeatureId } from './ids';

/**
 * Fallback tint color for the measured along-cave-line path (#26 ruler), used
 * when no configured color is available. The user-facing color lives in
 * `config.display.measure_path_color` (defaults to this value); a bright
 * cyan/teal, distinct from selection, hover, the snap indicator, and the
 * depth/distance gradient hues so a measured path reads clearly.
 */
export const MEASURE_TINT = '#22d3ee';

/**
 * Map the along-line path shots (from the `get_along_line_distance` IPC) to their
 * globally-unique MapLibre feature ids, for along-line tinting.
 * Reuses the one canonical `encodeFeatureId` formula so it never drifts from
 * feature construction or the distance-gradient overlay.
 */
export function buildMeasuredPathIds(pathShots: PathShot[]): Set<number> {
  const ids = new Set<number>();
  for (const s of pathShots) {
    ids.add(encodeFeatureId(s.line_id, s.shot_id));
  }
  return ids;
}

/**
 * Map the along-line path stations to their station feature ids (same
 * `encodeFeatureId` formula, scoped by `station_index`), so the path's stations
 * tint alongside its shots.
 */
export function buildMeasuredStationIds(pathStations: StationRef[]): Set<number> {
  const ids = new Set<number>();
  for (const s of pathStations) {
    ids.add(encodeFeatureId(s.line_id, s.station_index));
  }
  return ids;
}
