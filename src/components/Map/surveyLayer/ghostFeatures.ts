/**
 * Merge-preview ghost feature builder.
 *
 * Extracted from the (deleted) stock `features.ts` so the custom ghost layer
 * keeps its geometry source without depending on a stock-pipeline module. Turns
 * a `GhostLineData` snapshot into flat GeoJSON shot lines + station / merge
 * points for the merge-preview visualization.
 */
import type { GpsOrigin } from '../../../types/config';
import { metersToMapCoords } from '../../../utils/coordinates';
import type { GhostLineData } from './displayColor';

export interface GhostFeatureSets {
  shotFeatures: GeoJSON.Feature[];
  stationFeatures: GeoJSON.Feature[];
  mergeStationFeatures: GeoJSON.Feature[];
}

export function buildGhostFeatures(
  data: GhostLineData | null,
  gpsOrigin: GpsOrigin | null
): GhostFeatureSets {
  const shotFeatures: GeoJSON.Feature[] = [];
  const stationFeatures: GeoJSON.Feature[] = [];
  const mergeStationFeatures: GeoJSON.Feature[] = [];

  if (!data) {
    return { shotFeatures, stationFeatures, mergeStationFeatures };
  }

  for (const shot of data.shots) {
    shotFeatures.push({
      type: 'Feature',
      geometry: {
        type: 'LineString',
        coordinates: [
          metersToMapCoords(shot.from.x, shot.from.y, gpsOrigin),
          metersToMapCoords(shot.to.x, shot.to.y, gpsOrigin),
        ],
      },
      properties: { color: data.color },
    });
  }

  for (const station of data.stations) {
    stationFeatures.push({
      type: 'Feature',
      geometry: {
        type: 'Point',
        coordinates: metersToMapCoords(station.x, station.y, gpsOrigin),
      },
      properties: { color: data.color },
    });
  }

  if (data.mergeStation) {
    mergeStationFeatures.push({
      type: 'Feature',
      geometry: {
        type: 'Point',
        coordinates: metersToMapCoords(data.mergeStation.x, data.mergeStation.y, gpsOrigin),
      },
      properties: {},
    });
  }

  return { shotFeatures, stationFeatures, mergeStationFeatures };
}
