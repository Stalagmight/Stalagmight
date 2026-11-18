/**
 * The gas route highlight's GeoJSON (#233): one LineString per route leg, with
 * `isJump` flagged so jump legs draw faded (#243). A leg with an endpoint on a
 * line the map does not draw (a hidden Section's line, #327) is left out, like
 * the line itself.
 */
import type { RouteLeg } from '../../../types/ipc.generated';
import type { GpsOrigin } from '../../../types/config';
import type { Station, StationRef } from '../../../types/project';
import { metersToMapCoords } from '../../../utils/coordinates';

export function gasRouteFeatures(
  legs: readonly (RouteLeg & { color: string })[],
  stations: readonly Pick<Station, 'station_ref' | 'coordinates'>[],
  hiddenLines: ReadonlySet<number>,
  gpsOrigin: GpsOrigin | null,
) {
  // Index the stations once (keyed by line:index) so the per-leg endpoint lookups are O(1) -
  // the route can be long and this re-runs after every setData() rebuild, so a linear scan per
  // endpoint would be quadratic on large surveys.
  const coordByRef = new Map<string, [number, number]>();
  for (const s of stations) {
    if (hiddenLines.has(s.station_ref.line_id)) continue;
    coordByRef.set(
      `${s.station_ref.line_id}:${s.station_ref.station_index}`,
      metersToMapCoords(s.coordinates.x, s.coordinates.y, gpsOrigin) as [number, number],
    );
  }
  const coordFor = (ref: StationRef): [number, number] | null =>
    coordByRef.get(`${ref.line_id}:${ref.station_index}`) ?? null;
  return legs.flatMap((leg) => {
    const from = coordFor(leg.from);
    const to = coordFor(leg.to);
    if (!from || !to) return [];
    return [
      {
        type: 'Feature' as const,
        geometry: { type: 'LineString' as const, coordinates: [from, to] },
        properties: { isJump: leg.is_jump, segmentColor: leg.color },
      },
    ];
  });
}
