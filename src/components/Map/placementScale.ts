import type * as maplibregl from 'maplibre-gl';
import type { GpsOrigin } from '../../types/config';
import { mapCoordsToMeters, metersToMapCoords } from '../../utils/coordinates';

/**
 * Screen pixels per survey metre along each plan axis. With a GPS origin the
 * survey is drawn through ellipsoidal ENU, so in web mercator a metre east and a
 * metre north have different screen lengths (the prime-vertical vs the meridional
 * radius of curvature, up to 0.67% apart); one number for both put that whole
 * difference onto one axis (#576 RV4-FE-MF2).
 */
export interface PixelsPerMeter {
  east: number;
  north: number;
}

/** The initial scale before placement mode first measures the camera. */
export const UNIT_PIXELS_PER_METER: PixelsPerMeter = { east: 1, north: 1 };

/**
 * Screen pixels per survey metre at the map center, for the to-scale placement
 * preview. Computed on demand rather than on every zoom frame (#430).
 *
 * Measured as a 1 m step east and a 1 m step north through the SAME
 * metres-to-map transform the survey is drawn with (utils/coordinates.ts: WGS84
 * when the project has a GPS origin, the 111320 m/deg display convention when it
 * has none), so the preview and the drawn survey agree on both axes. A fixed
 * 1/111320 degree per metre was right only without an origin; on WGS84 a degree
 * of latitude is 110574-111694 m, so the preview was up to 0.7% off the survey it
 * is placed against (#576 NF-EXP-1).
 */
export function pixelsPerMeterAt(map: maplibregl.Map, gpsOrigin: GpsOrigin | null): PixelsPerMeter {
  const center = map.getCenter();
  const here = mapCoordsToMeters(center.lng, center.lat, gpsOrigin);
  const a = map.project(metersToMapCoords(here.x, here.y, gpsOrigin));
  const stepLength = (dx: number, dy: number): number => {
    const b = map.project(metersToMapCoords(here.x + dx, here.y + dy, gpsOrigin));
    const ppm = Math.hypot(b.x - a.x, b.y - a.y);
    return ppm > 0 ? ppm : 1;
  };
  return { east: stepLength(1, 0), north: stepLength(0, 1) };
}
