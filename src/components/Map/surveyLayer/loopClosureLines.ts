/**
 * The loop-closure indicator lines: one GeoJSON line per closure between its
 * two stations, carrying the colour the closure layer paints. The custom
 * renderer draws everything else; loop closures stay a small stock GeoJSON
 * overlay, like the gas route.
 */
import type { LoopClosure } from '../../../types/project';
import type { SurveyGeometryIndex } from './geometryIndex';

export function loopClosureFeatures(
  closures: readonly LoopClosure[],
  geoIndex: SurveyGeometryIndex,
  colorOf: (errorPercentage: number) => string,
): GeoJSON.Feature[] {
  const features: GeoJSON.Feature[] = [];
  for (const lc of closures) {
    const a = geoIndex.stationCoords(lc.station_a);
    const b = geoIndex.stationCoords(lc.station_b);
    if (!a || !b) continue;
    features.push({
      type: 'Feature',
      geometry: { type: 'LineString', coordinates: [a, b] },
      properties: {
        stationA: `${lc.station_a.line_id}:${lc.station_a.station_index}`,
        stationB: `${lc.station_b.line_id}:${lc.station_b.station_index}`,
        lineIdA: lc.station_a.line_id,
        lineIdB: lc.station_b.line_id,
        errorPercentage: lc.error_percentage,
        misclosureDistance: lc.misclosure_distance,
        pathLength: lc.path_length,
        color: colorOf(lc.error_percentage),
      },
    });
  }
  return features;
}
