import type { Station } from '../types/project';

/**
 * The station at `(lineId, stationIndex)` in a computed station list, or
 * `undefined` when the list has none there (a stale selection, a dangling
 * reference). Linear, like the inline scans it replaces; station lists in the
 * panels that call it are looked up a handful of times per render.
 */
export function findStation(
  stations: readonly Station[],
  lineId: number | undefined,
  stationIndex: number | undefined,
): Station | undefined {
  return stations.find(
    (s) => s.station_ref.line_id === lineId && s.station_ref.station_index === stationIndex,
  );
}
