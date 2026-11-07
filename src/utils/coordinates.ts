import type { Coordinates, GpsCoordinates } from '../types/project';
import type { GpsOrigin } from '../types/config';
import { formatFixed } from '../i18n/format';
import { normalizeAzimuth } from './azimuth';

// WGS84, the ellipsoid the backend places every zero datum on
// (`gps_to_local`, src-tauri/crates/models/src/geo.rs). The map, the data
// table's GPS column and GlobalView must invert THAT projection: this file used
// a 6371000 m sphere, which drew a datum ~3 m per km away from its own fix
// (#576 D36). Constants and operation order mirror geo.rs, so the two sides
// agree to well under 1e-9 degrees: the parity table holds geo.rs's
// `local_to_gps` to it within 1e-12 degrees and `enuToGps` within 1e-10
// (__tests__/coordinates.test.ts reads it from geo.rs).
const WGS84_A = 6378137.0;
const WGS84_E2 = 0.00669437999014;

// Meters per degree (approximately at equator)
// Used to transform local meter coordinates to pseudo-lat/lng for MapLibre
// when the project has no GPS origin - a display convention, not geodesy.
const METERS_PER_DEGREE = 111320;

const DEG_TO_RAD = Math.PI / 180;
const RAD_TO_DEG = 180 / Math.PI;

type Ecef = { x: number; y: number; z: number };
type Enu = { east: number; north: number; up: number };

type OriginFrame = {
  latRad: number;
  lonRad: number;
  sinLat: number;
  cosLat: number;
  sinLon: number;
  cosLon: number;
  ecef: Ecef;
};

function toRadians(deg: number): number {
  return deg * DEG_TO_RAD;
}

function toDegrees(rad: number): number {
  return rad * RAD_TO_DEG;
}

/** Geodetic (altitude 0) to ECEF on WGS84; mirrors `geodetic_to_ecef`. */
function gpsToEcef(latitude: number, longitude: number): Ecef {
  const latRad = toRadians(latitude);
  const lonRad = toRadians(longitude);
  const sinLat = Math.sin(latRad);
  const cosLat = Math.cos(latRad);
  // N = radius of curvature in the prime vertical
  const n = WGS84_A / Math.sqrt(1 - WGS84_E2 * sinLat * sinLat);
  return {
    x: n * cosLat * Math.cos(lonRad),
    y: n * cosLat * Math.sin(lonRad),
    z: n * (1 - WGS84_E2) * sinLat,
  };
}

// One-entry memo: every caller converts many points against the same origin
// (the renderer once per station), so the frame's trig is paid once per origin.
let cachedFrame: { latitude: number; longitude: number; frame: OriginFrame } | null = null;

function buildOriginFrame(origin: { latitude: number; longitude: number }): OriginFrame {
  if (
    cachedFrame !== null &&
    cachedFrame.latitude === origin.latitude &&
    cachedFrame.longitude === origin.longitude
  ) {
    return cachedFrame.frame;
  }
  const frame = computeOriginFrame(origin);
  cachedFrame = { latitude: origin.latitude, longitude: origin.longitude, frame };
  return frame;
}

function computeOriginFrame(origin: { latitude: number; longitude: number }): OriginFrame {
  const latRad = toRadians(origin.latitude);
  const lonRad = toRadians(origin.longitude);
  return {
    latRad,
    lonRad,
    sinLat: Math.sin(latRad),
    cosLat: Math.cos(latRad),
    sinLon: Math.sin(lonRad),
    cosLon: Math.cos(lonRad),
    ecef: gpsToEcef(origin.latitude, origin.longitude),
  };
}

function ecefToEnu(ecef: Ecef, origin: OriginFrame): Enu {
  const dx = ecef.x - origin.ecef.x;
  const dy = ecef.y - origin.ecef.y;
  const dz = ecef.z - origin.ecef.z;
  return {
    east: -origin.sinLon * dx + origin.cosLon * dy,
    north:
      -origin.sinLat * origin.cosLon * dx -
      origin.sinLat * origin.sinLon * dy +
      origin.cosLat * dz,
    up:
      origin.cosLat * origin.cosLon * dx +
      origin.cosLat * origin.sinLon * dy +
      origin.sinLat * dz,
  };
}

function gpsToEnu(
  gps: { latitude: number; longitude: number },
  origin: { latitude: number; longitude: number }
): Enu {
  const frame = buildOriginFrame(origin);
  const ecef = gpsToEcef(gps.latitude, gps.longitude);
  return ecefToEnu(ecef, frame);
}

// [#576 RV18-W28-1] The longest offset `surfacePoint` pins as given: 1e150 m,
// far beyond the horizon (about 6.4e6 m) and far below where squaring it
// overflows (about 1.3e154 m).
const PIN_MAX_OFFSET = 1e150;

/**
 * A finite offset longer than PIN_MAX_OFFSET, shortened to it in its own
 * direction (#576 RV18-W28-1). From about 1.3e154 m the solve's terms overflow
 * (qx * qx is Infinity) and the discriminant is NaN, which would turn a finite
 * offset into [NaN, NaN] on the map. Such an offset is beyond the horizon
 * whatever its length, and that far out the origin's own position is far below
 * rounding, so the shortened one pins exactly where the full length would.
 * Anything else, a non-finite input included, is returned as given.
 */
function shortenToPin(east: number, north: number): [number, number] {
  const longest = Math.max(Math.abs(east), Math.abs(north));
  if (!Number.isFinite(longest) || longest <= PIN_MAX_OFFSET) return [east, north];
  return [(east / longest) * PIN_MAX_OFFSET, (north / longest) * PIN_MAX_OFFSET];
}

/**
 * Where the origin's vertical through (`east`, `north`) on its tangent plane
 * meets the WGS84 surface, as latitude/longitude in degrees (longitude in
 * [-180, 180]). Mirrors `local_to_gps` in geo.rs operation for operation:
 * solve for the `t` that puts origin + east*E + north*N + t*U on the
 * ellipsoid (a quadratic; the root nearest the origin), then convert that
 * surface point to geodetic latitude, exact in closed form at height 0.
 *
 * `null` when the vertical misses the ellipsoid (an offset beyond the horizon,
 * roughly an earth radius out) or an input is not finite. With `pinToHorizon`,
 * a finite offset beyond the horizon instead takes the discriminant as zero:
 * the vertical's closest approach to the ellipsoid, a point above the surface,
 * so it lands near the horizon in (very nearly) its own direction, not on it.
 * The map's choice, see `metersToMapCoords`.
 */
function surfacePoint(
  eastGiven: number,
  northGiven: number,
  frame: OriginFrame,
  pinToHorizon: boolean
): { latitude: number; longitude: number } | null {
  const [east, north] = pinToHorizon
    ? shortenToPin(eastGiven, northGiven)
    : [eastGiven, northGiven];
  const b2 = WGS84_A * WGS84_A * (1 - WGS84_E2);
  const a2 = WGS84_A * WGS84_A;
  const { sinLat, cosLat, sinLon, cosLon } = frame;
  // ENU unit vectors at the origin, in ECEF.
  const ex = -sinLon;
  const ey = cosLon;
  const ez = 0;
  const nx = -sinLat * cosLon;
  const ny = -sinLat * sinLon;
  const nz = cosLat;
  const ux = cosLat * cosLon;
  const uy = cosLat * sinLon;
  const uz = sinLat;

  const qx = frame.ecef.x + east * ex + north * nx;
  const qy = frame.ecef.y + east * ey + north * ny;
  const qz = frame.ecef.z + east * ez + north * nz;
  // |(q + t*u)| on the ellipsoid: A t^2 + B t + C = 0.
  const qa = (ux * ux + uy * uy) / a2 + (uz * uz) / b2;
  const qb = 2 * ((qx * ux + qy * uy) / a2 + (qz * uz) / b2);
  const qc = (qx * qx + qy * qy) / a2 + (qz * qz) / b2 - 1;
  let disc = qb * qb - 4 * qa * qc;
  if (!Number.isFinite(disc)) return null;
  if (disc < 0) {
    if (!pinToHorizon) return null;
    disc = 0;
  }
  const sqrtDisc = Math.sqrt(disc);
  const t1 = (-qb + sqrtDisc) / (2 * qa);
  const t2 = (-qb - sqrtDisc) / (2 * qa);
  const t = Math.abs(t1) <= Math.abs(t2) ? t1 : t2;
  const x = qx + t * ux;
  const y = qy + t * uy;
  const z = qz + t * uz;

  // On the surface (height 0) geodetic latitude satisfies
  // tan(lat) = z / (p * (1 - e^2)) exactly.
  const p = Math.hypot(x, y);
  const latitude = toDegrees(Math.atan2(z, p * (1 - WGS84_E2)));
  const longitude = toDegrees(Math.atan2(y, x));
  return Number.isFinite(latitude) && Number.isFinite(longitude) ? { latitude, longitude } : null;
}

/**
 * The inverse of the backend's `gps_to_local`: the point ON the WGS84
 * ellipsoid whose east/north from `origin` are (`east`, `north`), longitude in
 * [-180, 180]. The one local-to-GPS algorithm (#576 GEO-D1): `local_to_gps` in
 * geo.rs runs it, so the workspace map, the GPS column and GlobalView place a
 * station identically (#576 D36); the two agree to well under 1e-9 degrees
 * (the parity table in __tests__/coordinates.test.ts and geo.rs). Exact to
 * rounding: the round trip through `gps_to_local` is within nanometres.
 *
 * `null` beyond the horizon (roughly an earth radius out) or for a non-finite
 * input, as the backend's `None`.
 */
export function enuToGps(
  east: number,
  north: number,
  origin: { latitude: number; longitude: number }
): { latitude: number; longitude: number } | null {
  return surfacePoint(east, north, buildOriginFrame(origin), false);
}

/**
 * `lon` (degrees, as `atan2` returns it in [-180, 180]) moved by a whole turn
 * when that brings it within half a turn of `reference`; mirrors
 * `unwrap_longitude` in geo.rs. Away from the antimeridian it is returned
 * unchanged, bit for bit. The explicit step for drawing only: the result can
 * leave [-180, 180], so it is never a stored or displayed fix.
 */
function unwrapLongitude(lon: number, reference: number): number {
  const delta = lon - reference;
  if (delta > 180) return lon - 360;
  if (delta < -180) return lon + 360;
  return lon;
}

/**
 * Transform local meter coordinates to MapLibre-compatible lat/lng
 * If gpsOrigin is provided, coordinates are offset from that origin.
 * This allows satellite tiles to align with real-world geography.
 *
 * The position is `enuToGps`'s (#576 GEO-D1), with the longitude then made
 * continuous with the origin's (`unwrapLongitude`), as GlobalView draws it
 * (survey_geometry.rs), so it can leave [-180, 180] for a survey that crosses
 * the antimeridian: 1000 m east of 179.999 is drawn at 180.0084, not at
 * -179.9916, which the map would join to the origin the long way round the
 * globe (#576 D36 review).
 *
 * Beyond the horizon (roughly an earth radius from the origin; no survey gets
 * there, only corrupt coordinates) there is no point on the earth to draw. The
 * map still needs a finite one - a NaN would break its camera and bounds - so
 * the point is pinned near the horizon in its own direction, where the rest of
 * the survey shows a line heading off towards it. GlobalView refuses such a
 * project instead, and the GPS column leaves the cell blank. A non-finite
 * input gives [NaN, NaN], as it always did.
 */
export function metersToMapCoords(
  x: number,
  y: number,
  gpsOrigin?: GpsOrigin | null
): [number, number] {
  if (!gpsOrigin) {
    // Fallback pseudo-lat/lng for local-only mode (no GPS origin).
    const metersPerDegreeLng = METERS_PER_DEGREE;
    const lng = x / metersPerDegreeLng;
    const lat = y / METERS_PER_DEGREE;
    return [lng, lat];
  }

  const gps = surfacePoint(x, y, buildOriginFrame(gpsOrigin), true);
  if (gps === null) return [NaN, NaN];
  return [unwrapLongitude(gps.longitude, gpsOrigin.longitude), gps.latitude];
}

/**
 * Transform MapLibre lat/lng back to local meter coordinates
 * If gpsOrigin is provided, coordinates are relative to that origin.
 */
export function mapCoordsToMeters(
  lng: number,
  lat: number,
  gpsOrigin?: GpsOrigin | null
): { x: number; y: number } {
  if (!gpsOrigin) {
    return {
      x: lng * METERS_PER_DEGREE,
      y: lat * METERS_PER_DEGREE,
    };
  }

  const enu = gpsToEnu({ latitude: lat, longitude: lng }, gpsOrigin);
  return { x: enu.east, y: enu.north };
}

/**
 * Convert GPS coordinates to internal XY coordinates (meters from origin)
 */
export function gpsToXY(
  gps: GpsCoordinates,
  origin: GpsCoordinates
): Coordinates {
  const enu = gpsToEnu(gps, origin);
  return { x: enu.east, y: enu.north, z: gps.depth_reference };
}

/**
 * Convert internal XY coordinates to GPS coordinates: `enuToGps`, so the
 * longitude is the conventional one in [-180, 180]. `null` beyond the horizon
 * (or for a non-finite input), where no point on the earth has these
 * coordinates; the data table's GPS column then shows an empty cell.
 */
export function xyToGps(
  coords: Coordinates,
  origin: GpsCoordinates
): GpsCoordinates | null {
  const gps = enuToGps(coords.x, coords.y, origin);
  if (gps === null) return null;
  return { latitude: gps.latitude, longitude: gps.longitude, depth_reference: coords.z };
}

/**
 * Calculate distance between two coordinates
 */
export function distance(a: Coordinates, b: Coordinates): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const dz = b.z - a.z;
  return Math.sqrt(dx * dx + dy * dy + dz * dz);
}

/**
 * Calculate horizontal distance between two coordinates
 */
export function horizontalDistance(a: Coordinates, b: Coordinates): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  return Math.sqrt(dx * dx + dy * dy);
}

/**
 * Calculate the bounding box of a set of coordinates
 */
export function getBounds(coords: Coordinates[]): {
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
  minZ: number;
  maxZ: number;
} {
  if (coords.length === 0) {
    return { minX: 0, maxX: 0, minY: 0, maxY: 0, minZ: 0, maxZ: 0 };
  }

  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  let minZ = Infinity;
  let maxZ = -Infinity;

  for (const coord of coords) {
    minX = Math.min(minX, coord.x);
    maxX = Math.max(maxX, coord.x);
    minY = Math.min(minY, coord.y);
    maxY = Math.max(maxY, coord.y);
    minZ = Math.min(minZ, coord.z);
    maxZ = Math.max(maxZ, coord.z);
  }

  return { minX, maxX, minY, maxY, minZ, maxZ };
}

/**
 * Bounds over an interleaved `[x,y,z, x,y,z, …]` station-coordinate buffer (the
 * SoA `SurveyData.stationCoords`) - equivalent to `getBounds` but WITHOUT
 * materializing a `Coordinates[]`. The initial-view sizing used to
 * `stations.map((s) => s.coordinates)` the lazy `Station[]` proxy, which rebuilt
 * the whole ~89k-object graph on load just to read coordinates (#mem). Empty
 * buffer -> zeros, matching `getBounds([])`.
 */
export function getBoundsFromStationCoords(stationCoords: Float64Array): {
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
  minZ: number;
  maxZ: number;
} {
  if (stationCoords.length === 0) {
    return { minX: 0, maxX: 0, minY: 0, maxY: 0, minZ: 0, maxZ: 0 };
  }
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  let minZ = Infinity;
  let maxZ = -Infinity;
  for (let i = 0; i + 3 <= stationCoords.length; i += 3) {
    const x = stationCoords[i]!;
    const y = stationCoords[i + 1]!;
    const z = stationCoords[i + 2]!;
    minX = Math.min(minX, x);
    maxX = Math.max(maxX, x);
    minY = Math.min(minY, y);
    maxY = Math.max(maxY, y);
    minZ = Math.min(minZ, z);
    maxZ = Math.max(maxZ, z);
  }
  return { minX, maxX, minY, maxY, minZ, maxZ };
}

/**
 * Calculate azimuth from point A to point B (degrees, [0, 360))
 *
 * Wrapped through `normalizeAzimuth` rather than a local `if (angle < 0) angle
 * += 360`: for a hair-west-of-north displacement that addend rounds to exactly
 * `360`, which is out of range and trips a non-ignorable `INVALID_AZIMUTH`.
 */
export function calculateAzimuth(from: Coordinates, to: Coordinates): number {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  return normalizeAzimuth(Math.atan2(dx, dy) * (180 / Math.PI));
}

/**
 * Calculate new position from a point given azimuth and distance
 */
export function calculatePosition(
  from: Coordinates,
  azimuth: number,
  distance: number,
  depthChange: number
): Coordinates {
  const azimuthRad = (azimuth * Math.PI) / 180;
  return {
    x: from.x + distance * Math.sin(azimuthRad),
    y: from.y + distance * Math.cos(azimuthRad),
    z: from.z + depthChange,
  };
}

/**
 * Format a coordinate for display
 */
export function formatCoordinate(value: number, precision: number = 1): string {
  return formatFixed(value, undefined, precision);
}

/**
 * Format a GPS coordinate for display
 */
export function formatGps(gps: GpsCoordinates, precision: number = 6): string {
  return `${formatFixed(gps.latitude, undefined, precision)}, ${formatFixed(gps.longitude, undefined, precision)}`;
}
