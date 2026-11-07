import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  enuToGps,
  metersToMapCoords,
  mapCoordsToMeters,
  gpsToXY,
  xyToGps,
  distance,
  horizontalDistance,
  getBounds,
  getBoundsFromStationCoords,
  calculateAzimuth,
  calculatePosition,
  formatCoordinate,
  formatGps,
} from '../coordinates';
import type { Coordinates, GpsCoordinates } from '../../types/project';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function coord(x: number, y: number, z: number): Coordinates {
  return { x, y, z };
}

function gps(lat: number, lng: number, depth: number): GpsCoordinates {
  return { latitude: lat, longitude: lng, depth_reference: depth };
}

const EQUATOR_ORIGIN = { latitude: 0, longitude: 0 };

// ---------------------------------------------------------------------------
// metersToMapCoords / mapCoordsToMeters - no GPS origin (pseudo mode)
// ---------------------------------------------------------------------------

describe('metersToMapCoords (pseudo mode)', () => {
  it('converts origin to (0,0)', () => {
    const [lng, lat] = metersToMapCoords(0, 0);
    expect(lng).toBe(0);
    expect(lat).toBe(0);
  });

  it('converts 111320m east to ~1° longitude', () => {
    const [lng] = metersToMapCoords(111320, 0);
    expect(lng).toBeCloseTo(1.0, 3);
  });

  it('converts 111320m north to ~1° latitude', () => {
    const [, lat] = metersToMapCoords(0, 111320);
    expect(lat).toBeCloseTo(1.0, 3);
  });
});

describe('mapCoordsToMeters (pseudo mode)', () => {
  it('converts (0,0) to origin', () => {
    const { x, y } = mapCoordsToMeters(0, 0);
    expect(x).toBe(0);
    expect(y).toBe(0);
  });

  it('round-trips with metersToMapCoords', () => {
    const [lng, lat] = metersToMapCoords(500, -300);
    const { x, y } = mapCoordsToMeters(lng, lat);
    expect(x).toBeCloseTo(500, 3);
    expect(y).toBeCloseTo(-300, 3);
  });
});

// ---------------------------------------------------------------------------
// metersToMapCoords / mapCoordsToMeters - with GPS origin
// ---------------------------------------------------------------------------

describe('metersToMapCoords (GPS mode)', () => {
  it('maps origin meters to origin GPS', () => {
    const origin = { latitude: 45, longitude: -90 };
    const [lng, lat] = metersToMapCoords(0, 0, origin);
    expect(lng).toBeCloseTo(-90, 3);
    expect(lat).toBeCloseTo(45, 3);
  });

  it('100m east from equator origin shifts longitude', () => {
    const [lng, lat] = metersToMapCoords(100, 0, EQUATOR_ORIGIN);
    // 100m east at equator is a small fraction of a degree
    expect(lng).toBeGreaterThan(0);
    expect(lng).toBeLessThan(0.01);
    expect(lat).toBeCloseTo(0, 3);
  });

  it('round-trips through mapCoordsToMeters', () => {
    const origin = { latitude: 30, longitude: 10 };
    const [lng, lat] = metersToMapCoords(250, -150, origin);
    const { x, y } = mapCoordsToMeters(lng, lat, origin);
    expect(x).toBeCloseTo(250, 0);
    expect(y).toBeCloseTo(-150, 0);
  });
});

// ---------------------------------------------------------------------------
// One earth model with the backend (#576 D36)
// ---------------------------------------------------------------------------

// [origin lat, origin lon, target lat, target lon, east m, north m]. east/north
// are what the backend's gps_to_local (crates/models/src/geo.rs, WGS84) returns
// for the target - i.e. where the backend PLACES a zero datum typed with that
// GPS fix. The same table is asserted against the Rust side in
// src-tauri/src/commands/survey_geometry.rs, so the map, the GPS column and
// GlobalView all invert the one model the backend projects with.
const BACKEND_VECTORS: Array<[number, number, number, number, number, number]> = [
  [30.0, 0.0, 30.009, 0.0, 0.0, 997.6726609534434],
  [30.0, 0.0, 30.0, 0.009, 868.3765186870131, 0.03410106617957353],
  [0.0, 0.0, 0.009, 0.0, 0.0, 995.1684783840738],
  [0.0, 0.0, 0.0, 0.009, 1001.8754130194145, 0.0],
  [45.0, -90.0, 45.05, -90.05, -3938.910955978453, 5557.827879505819],
  [-30.0, 151.0, -30.18, 151.18, 17336.07348511051, -19967.295913146296],
  [60.0, 25.0, 60.1, 24.9, -5563.148767087514, 11145.512358657026],
  [20.5, -87.3, 20.45, -87.2, 10434.644415801886, -5532.324088892108],
  [70.0, 0.0, 70.02, 0.3, 11444.93118591173, 2259.3988331612372],
  [-54.8, -68.3, -54.9, -68.1, 12830.611701495578, -11150.36972160698],
];

/** Ground distance between two nearby fixes, in metres (for readable failures). */
function metresApart(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const north = (lat2 - lat1) * 111_132;
  const east = (lon2 - lon1) * 111_320 * Math.cos((lat1 * Math.PI) / 180);
  return Math.hypot(north, east);
}

describe('WGS84 parity with the backend (#576 D36)', () => {
  it.each(BACKEND_VECTORS)(
    'origin (%d, %d): a station at the backend position of (%d, %d) is drawn on that fix',
    (olat, olon, lat, lon, east, north) => {
      const origin = { latitude: olat, longitude: olon };
      const [lng2, lat2] = metersToMapCoords(east, north, origin);
      const off = metresApart(lat, lon, lat2, lng2);
      expect(off, `drawn ${off.toFixed(3)} m from its own fix`).toBeLessThan(0.001);
      expect(Math.abs(lat2 - lat)).toBeLessThan(1e-9);
      expect(Math.abs(lng2 - lon)).toBeLessThan(1e-9);
    },
  );

  it.each(BACKEND_VECTORS)(
    'origin (%d, %d): the GPS column shows (%d, %d) for that station',
    (olat, olon, lat, lon, east, north) => {
      const back = xyToGps(coord(east, north, 0), gps(olat, olon, 0))!;
      const off = metresApart(lat, lon, back.latitude, back.longitude);
      expect(off, `GPS column ${off.toFixed(3)} m off`).toBeLessThan(0.001);
    },
  );

  it.each(BACKEND_VECTORS)(
    'origin (%d, %d): a click on (%d, %d) maps to the local metres the backend uses',
    (olat, olon, lat, lon, east, north) => {
      const m = mapCoordsToMeters(lon, lat, { latitude: olat, longitude: olon });
      const off = Math.hypot(m.x - east, m.y - north);
      expect(off, `click lands ${off.toFixed(3)} m from the backend's position`).toBeLessThan(1e-6);
    },
  );
});

// A survey across the antimeridian is drawn in one piece (#576 D36 review
// RV1-RUST-5): from an origin at 179.999 E, a station 1000 m east is drawn at
// 180.0084 - continuous with the origin, as the backend's local_to_gps returns
// it for GlobalView - not at -179.9916, which the map joins to the origin by a
// line the long way round the globe. The GPS column keeps showing the
// conventional [-180, 180) longitude.
describe('across the antimeridian', () => {
  it.each([
    [179.999, 1000],
    [-179.999, -1000],
  ])('origin lon %d: a station %d m east is drawn next to the origin', (olon, east) => {
    const origin = { latitude: -16.8, longitude: olon };
    const [lng, lat] = metersToMapCoords(east, 0, origin);
    expect(Math.abs(lng - olon), `drawn at lng ${lng}`).toBeLessThan(0.01);
    const m = mapCoordsToMeters(lng, lat, origin);
    expect(Math.hypot(m.x - east, m.y)).toBeLessThan(1e-6);
  });

  it('the GPS column still shows the conventional longitude', () => {
    const back = xyToGps(coord(1000, 0, 0), gps(-16.8, 179.999, 0))!;
    expect(back.longitude).toBeCloseTo(-179.991619, 6);
  });
});

// ---------------------------------------------------------------------------
// One local-to-GPS algorithm with the backend (#576 GEO-D1)
// ---------------------------------------------------------------------------

// The backend/frontend differential: `FRONTEND_PARITY` in geo.rs, read from
// source (the repo's source-scan idiom, as knownFileExtensionsParity.test.ts),
// so there is one table. Each row is `origin_lat origin_lon east_m north_m lat
// lon`, the last two what the backend's `local_to_gps` returns, or `none none`
// where it is `None`; geo.rs pins it to the backend to 1e-12 degrees
// (`local_to_gps_matches_the_frontend_parity_table`), and enuToGps is held to
// it here to 1e-10, so the two agree to within 1e-9 degrees. Offsets of about
// 0.1, 1, 5, 20 and 50 km at the equator, at a mid latitude, near both poles
// and on both sides of the antimeridian, then out to and past the horizon.
const GEO_RS = 'src-tauri/crates/models/src/geo.rs';

type ParityRow = [number, number, number, number];

function backendParityRows(): { fixes: Array<[...ParityRow, number, number]>; beyond: ParityRow[] } {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../');
  const source = fs.readFileSync(path.join(root, GEO_RS), 'utf8');
  const match = /const FRONTEND_PARITY: &str = "([^"]*)";/.exec(source);
  if (!match) throw new Error(`${GEO_RS}: no FRONTEND_PARITY table - renamed? Update this test with it.`);
  const fixes: Array<[...ParityRow, number, number]> = [];
  const beyond: ParityRow[] = [];
  for (const line of match[1]!.split('\n')) {
    const field = line.trim().split(/\s+/);
    if (field.length === 1 && field[0] === '') continue;
    if (field.length !== 6) throw new Error(`${GEO_RS}: a parity row has six fields: ${line}`);
    const [olat, olon, east, north] = field.slice(0, 4).map(Number) as ParityRow;
    if (field[4] === 'none') beyond.push([olat, olon, east, north]);
    else fixes.push([olat, olon, east, north, Number(field[4]), Number(field[5])]);
  }
  if (fixes.length < 30 || beyond.length < 3) throw new Error(`${GEO_RS}: the parity table lost its rows`);
  return { fixes, beyond };
}

const { fixes: PARITY, beyond: BEYOND_HORIZON } = backendParityRows();

describe('one local-to-GPS algorithm with the backend (#576 GEO-D1)', () => {
  it.each(PARITY)(
    'origin (%d, %d), offset (%d, %d) m: enuToGps returns the backend fix',
    (olat, olon, east, north, lat, lon) => {
      const got = enuToGps(east, north, { latitude: olat, longitude: olon });
      expect(got).not.toBeNull();
      expect(Math.abs(got!.latitude - lat), `lat ${got!.latitude} vs ${lat}`).toBeLessThan(1e-10);
      expect(Math.abs(got!.longitude - lon), `lon ${got!.longitude} vs ${lon}`).toBeLessThan(1e-10);
    },
  );

  it.each(BEYOND_HORIZON)(
    'origin (%d, %d), offset (%d, %d) m: beyond the horizon, as the backend None',
    (olat, olon, east, north) => {
      expect(enuToGps(east, north, { latitude: olat, longitude: olon })).toBeNull();
      expect(xyToGps(coord(east, north, 0), gps(olat, olon, 0))).toBeNull();
    },
  );

  it('a non-finite offset has no fix', () => {
    expect(enuToGps(Number.NaN, 0, EQUATOR_ORIGIN)).toBeNull();
    expect(enuToGps(0, Number.POSITIVE_INFINITY, EQUATOR_ORIGIN)).toBeNull();
    expect(metersToMapCoords(Number.NaN, 0, EQUATOR_ORIGIN).every(Number.isNaN)).toBe(true);
  });

  // The map needs a finite point even for a corrupt station: beyond the
  // horizon it is pinned near the horizon in its own direction (on it here:
  // due east of (0, 0) the horizon is longitude 90, one semi-major axis out).
  it('the map pins a point beyond the horizon on the horizon', () => {
    const [inside] = metersToMapCoords(6_378_000, 0, EQUATOR_ORIGIN);
    expect(inside).toBeCloseTo(89.62446, 4);
    for (const east of [6_379_000, 20_000_000, 1e9]) {
      const [lng, lat] = metersToMapCoords(east, 0, EQUATOR_ORIGIN);
      expect(lng, `${east} m east`).toBeCloseTo(90, 9);
      expect(lat, `${east} m east`).toBeCloseTo(0, 9);
    }
    const [lng, lat] = metersToMapCoords(-1e7, 1e7, { latitude: 20.5, longitude: -87.3 });
    expect(Number.isFinite(lng) && Number.isFinite(lat)).toBe(true);
  });

  // [#576 RV18-W28-1] From about 1.3e154 m the quadratic's terms overflow
  // (qx * qx is Infinity), and the pin must still hold: such an offset is
  // beyond the horizon whatever its length, so it lands where a merely far one
  // in the same direction does, not at [NaN, NaN]. Only a non-finite input
  // gives NaN.
  it('the map pins a finite offset too large to square, as a far one', () => {
    const MID = { latitude: 20.5, longitude: -87.3 };
    for (const [origin, east, north] of [
      [EQUATOR_ORIGIN, 1e160, 0],
      [EQUATOR_ORIGIN, Number.MAX_VALUE, Number.MAX_VALUE],
      [MID, 1e160, 0],
      [MID, -1e200, 3e199],
      [MID, -Number.MAX_VALUE, -Number.MAX_VALUE],
    ] as const) {
      const scale = 1e150 / Math.max(Math.abs(east), Math.abs(north));
      const far = metersToMapCoords(east * scale, north * scale, origin);
      const huge = metersToMapCoords(east, north, origin);
      expect(huge.every(Number.isFinite), `(${east}, ${north}) m: ${huge}`).toBe(true);
      expect(huge[0], `(${east}, ${north}) m`).toBeCloseTo(far[0], 9);
      expect(huge[1], `(${east}, ${north}) m`).toBeCloseTo(far[1], 9);
    }
    const [lng, lat] = metersToMapCoords(1e160, 0, EQUATOR_ORIGIN);
    expect(lng).toBeCloseTo(90, 9);
    expect(lat).toBeCloseTo(0, 9);
    expect(metersToMapCoords(Number.POSITIVE_INFINITY, 0, MID).every(Number.isNaN)).toBe(true);
  });

  // The frontend twin of geo.rs `local_to_gps_round_trips_within_a_
  // micrometre_out_to_50_km`: what the map draws converts back to the metres
  // it was drawn from, within 1e-6 m, anywhere within 50 km.
  it('round-trips through mapCoordsToMeters within 1e-6 m out to 50 km', () => {
    const origins = [
      { latitude: 0, longitude: 0 },
      { latitude: 20.5, longitude: -87.3 },
      { latitude: 89.9, longitude: 10 },
      { latitude: -89.99, longitude: -45 },
      { latitude: -16.8, longitude: 179.999 },
    ];
    let worst = 0;
    for (const origin of origins) {
      for (const d of [0.1, 1, 1_000, 5_000, 20_000, 50_000]) {
        for (let k = 0; k < 16; k++) {
          const a = (k * Math.PI) / 8 + 0.1;
          const [east, north] = [d * Math.sin(a), d * Math.cos(a)];
          const [lng, lat] = metersToMapCoords(east, north, origin);
          const m = mapCoordsToMeters(lng, lat, origin);
          worst = Math.max(worst, Math.hypot(m.x - east, m.y - north));
        }
      }
    }
    expect(worst).toBeLessThan(1e-6);
  });
});

// ---------------------------------------------------------------------------
// gpsToXY / xyToGps round-trip
// ---------------------------------------------------------------------------

describe('gpsToXY / xyToGps', () => {
  it('returns origin for same point', () => {
    const origin = gps(45, -90, 0);
    const xy = gpsToXY(origin, origin);
    expect(xy.x).toBeCloseTo(0, 1);
    expect(xy.y).toBeCloseTo(0, 1);
  });

  it('preserves depth_reference in z', () => {
    const point = gps(45, -90, 42);
    const origin = gps(45, -90, 0);
    const xy = gpsToXY(point, origin);
    expect(xy.z).toBe(42);
  });

  it('round-trips through xyToGps', () => {
    const origin = gps(10, 20, 0);
    const point = gps(10.001, 20.001, 5);
    const xy = gpsToXY(point, origin);
    const back = xyToGps(xy, origin)!;
    expect(back.latitude).toBeCloseTo(10.001, 3);
    expect(back.longitude).toBeCloseTo(20.001, 3);
    expect(back.depth_reference).toBe(5);
  });
});

// ---------------------------------------------------------------------------
// distance
// ---------------------------------------------------------------------------

describe('distance', () => {
  it('returns 0 for same point', () => {
    expect(distance(coord(1, 2, 3), coord(1, 2, 3))).toBe(0);
  });

  it('calculates 3D Euclidean distance', () => {
    // 3-4-5 triangle in XY, then Z=0
    expect(distance(coord(0, 0, 0), coord(3, 4, 0))).toBeCloseTo(5, 10);
  });

  it('includes Z component', () => {
    // sqrt(1 + 1 + 1) = sqrt(3) ≈ 1.732
    expect(distance(coord(0, 0, 0), coord(1, 1, 1))).toBeCloseTo(Math.sqrt(3), 10);
  });

  it('handles negative coordinates', () => {
    expect(distance(coord(-5, -5, -5), coord(5, 5, 5))).toBeCloseTo(Math.sqrt(300), 5);
  });
});

// ---------------------------------------------------------------------------
// horizontalDistance
// ---------------------------------------------------------------------------

describe('horizontalDistance', () => {
  it('ignores Z component', () => {
    expect(horizontalDistance(coord(0, 0, 0), coord(3, 4, 100))).toBeCloseTo(5, 10);
  });

  it('returns 0 for same XY', () => {
    expect(horizontalDistance(coord(1, 2, 0), coord(1, 2, 99))).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// getBounds
// ---------------------------------------------------------------------------

describe('getBounds', () => {
  it('returns zeros for empty array', () => {
    const b = getBounds([]);
    expect(b).toEqual({ minX: 0, maxX: 0, minY: 0, maxY: 0, minZ: 0, maxZ: 0 });
  });

  it('returns point for single coordinate', () => {
    const b = getBounds([coord(5, -3, 2)]);
    expect(b.minX).toBe(5);
    expect(b.maxX).toBe(5);
    expect(b.minY).toBe(-3);
    expect(b.maxY).toBe(-3);
    expect(b.minZ).toBe(2);
    expect(b.maxZ).toBe(2);
  });

  it('computes correct bounds for multiple coordinates', () => {
    const b = getBounds([
      coord(-10, 5, 0),
      coord(20, -15, 3),
      coord(0, 0, -1),
    ]);
    expect(b.minX).toBe(-10);
    expect(b.maxX).toBe(20);
    expect(b.minY).toBe(-15);
    expect(b.maxY).toBe(5);
    expect(b.minZ).toBe(-1);
    expect(b.maxZ).toBe(3);
  });
});

describe('getBoundsFromStationCoords', () => {
  it('returns zeros for an empty buffer (matches getBounds([]))', () => {
    expect(getBoundsFromStationCoords(new Float64Array(0))).toEqual(getBounds([]));
  });

  it('matches getBounds over the same coordinates without materializing objects', () => {
    const pts = [coord(-10, 5, 0), coord(20, -15, 3), coord(0, 0, -1)];
    const flat = new Float64Array(pts.flatMap((p) => [p.x, p.y, p.z]));
    expect(getBoundsFromStationCoords(flat)).toEqual(getBounds(pts));
  });

  it('handles a single station', () => {
    expect(getBoundsFromStationCoords(new Float64Array([5, -3, 2]))).toEqual(getBounds([coord(5, -3, 2)]));
  });
});

// ---------------------------------------------------------------------------
// calculateAzimuth
// ---------------------------------------------------------------------------

describe('calculateAzimuth', () => {
  it('north is 0°', () => {
    // Moving in +Y direction = north = 0°
    expect(calculateAzimuth(coord(0, 0, 0), coord(0, 10, 0))).toBeCloseTo(0, 5);
  });

  it('east is 90°', () => {
    expect(calculateAzimuth(coord(0, 0, 0), coord(10, 0, 0))).toBeCloseTo(90, 5);
  });

  it('south is 180°', () => {
    expect(calculateAzimuth(coord(0, 0, 0), coord(0, -10, 0))).toBeCloseTo(180, 5);
  });

  it('west is 270°', () => {
    expect(calculateAzimuth(coord(0, 0, 0), coord(-10, 0, 0))).toBeCloseTo(270, 5);
  });

  it('northeast is 45°', () => {
    expect(calculateAzimuth(coord(0, 0, 0), coord(10, 10, 0))).toBeCloseTo(45, 5);
  });

  it('result is always in [0, 360)', () => {
    const result = calculateAzimuth(coord(5, 5, 0), coord(0, 0, 0));
    expect(result).toBeGreaterThanOrEqual(0);
    expect(result).toBeLessThan(360);
  });
});

// ---------------------------------------------------------------------------
// calculatePosition
// ---------------------------------------------------------------------------

describe('calculatePosition', () => {
  it('north at 0° moves in +Y', () => {
    const pos = calculatePosition(coord(0, 0, 0), 0, 10, 0);
    expect(pos.x).toBeCloseTo(0, 5);
    expect(pos.y).toBeCloseTo(10, 5);
    expect(pos.z).toBe(0);
  });

  it('east at 90° moves in +X', () => {
    const pos = calculatePosition(coord(0, 0, 0), 90, 10, 0);
    expect(pos.x).toBeCloseTo(10, 5);
    expect(pos.y).toBeCloseTo(0, 5);
  });

  it('applies depth change to Z', () => {
    const pos = calculatePosition(coord(0, 0, 5), 0, 10, 3);
    expect(pos.z).toBe(8);
  });

  it('round-trips with calculateAzimuth', () => {
    const from = coord(10, 20, 0);
    const azimuth = 135;
    const dist = 15;
    const to = calculatePosition(from, azimuth, dist, 0);
    const backAzimuth = calculateAzimuth(from, to);
    expect(backAzimuth).toBeCloseTo(azimuth, 5);
  });

  it('starts from arbitrary position', () => {
    const pos = calculatePosition(coord(100, 200, 50), 0, 5, -2);
    expect(pos.x).toBeCloseTo(100, 5);
    expect(pos.y).toBeCloseTo(205, 5);
    expect(pos.z).toBe(48);
  });
});

// ---------------------------------------------------------------------------
// formatCoordinate / formatGps
// ---------------------------------------------------------------------------

describe('formatCoordinate', () => {
  it('formats with default precision', () => {
    const result = formatCoordinate(12.345);
    expect(result).toBe('12.3');
  });

  it('formats with custom precision', () => {
    const result = formatCoordinate(12.345, 2);
    expect(result).toBe('12.35');
  });
});

describe('formatGps', () => {
  it('formats lat/lng with default precision', () => {
    const result = formatGps(gps(45.123456, -90.654321, 0));
    expect(result).toBe('45.123456, -90.654321');
  });

  it('formats with custom precision', () => {
    const result = formatGps(gps(45.1, -90.2, 0), 2);
    expect(result).toBe('45.10, -90.20');
  });
});
