/**
 * The to-scale placement preview's pixels-per-metre (#576 NF-EXP-1). It must equal
 * the scale the survey itself is drawn at, which is the metres-to-map transform in
 * utils/coordinates.ts: WGS84 when the project has a GPS origin, the 111320 m/deg
 * display convention when it has none. A fixed 1/111320 deg per metre was right
 * only for the latter; on WGS84 a degree of latitude is 110574-111694 m.
 */
import { describe, expect, it } from 'vitest';
import type * as maplibregl from 'maplibre-gl';
import { pixelsPerMeterAt } from '../placementScale';
import { mapCoordsToMeters, metersToMapCoords } from '../../../utils/coordinates';

/** A map stand-in with MapLibre's web-mercator `project` at a fixed zoom. */
function fakeMercatorMap(center: { lng: number; lat: number }, zoom: number): maplibregl.Map {
  const worldSize = 512 * 2 ** zoom;
  const toPx = (lng: number, lat: number) => {
    const x = ((lng + 180) / 360) * worldSize;
    const phi = (lat * Math.PI) / 180;
    const y = ((1 - Math.log(Math.tan(Math.PI / 4 + phi / 2)) / Math.PI) / 2) * worldSize;
    return { x, y };
  };
  return {
    getCenter: () => center,
    project: (p: [number, number] | { lng: number; lat: number }) =>
      Array.isArray(p) ? toPx(p[0], p[1]) : toPx(p.lng, p.lat),
  } as unknown as maplibregl.Map;
}

/**
 * The drawn survey's own screen length of a 1 m step east and a 1 m step north at
 * the map center. Both axes: with a GPS origin they differ (#576 RV4-FE-MF2), and
 * an oracle that measured only north could not see a scale that was wrong east.
 */
function drawnPixelsPerMeter(
  map: maplibregl.Map,
  origin: { latitude: number; longitude: number } | null,
): { east: number; north: number } {
  const c = map.getCenter();
  const here = mapCoordsToMeters(c.lng, c.lat, origin);
  const a = map.project(metersToMapCoords(here.x, here.y, origin));
  const e = map.project(metersToMapCoords(here.x + 1, here.y, origin));
  const n = map.project(metersToMapCoords(here.x, here.y + 1, origin));
  return { east: Math.hypot(e.x - a.x, e.y - a.y), north: Math.hypot(n.x - a.x, n.y - a.y) };
}

const relErr = (got: number, want: number) => Math.abs(got - want) / want;

describe('pixelsPerMeterAt', () => {
  const origins = [
    { name: 'equator', latitude: 0.5, longitude: 10 },
    { name: 'mid latitude', latitude: 45, longitude: -80 },
    { name: 'high latitude', latitude: 70, longitude: 20 },
  ];

  it.each(origins)('matches the drawn survey scale with a GPS origin ($name)', (o) => {
    const origin = { latitude: o.latitude, longitude: o.longitude };
    const map = fakeMercatorMap({ lng: o.longitude + 0.001, lat: o.latitude + 0.001 }, 18);
    const expected = drawnPixelsPerMeter(map, origin);
    const got = pixelsPerMeterAt(map, origin);
    expect(relErr(got.east, expected.east)).toBeLessThan(1e-6);
    expect(relErr(got.north, expected.north)).toBeLessThan(1e-6);
  });

  it('tells east from north with a GPS origin away from the poles', () => {
    // N/M - 1 at 0.5 deg is ~0.67%: one number could not serve both axes.
    const origin = { latitude: 0.5, longitude: 10 };
    const map = fakeMercatorMap({ lng: 10.001, lat: 0.501 }, 18);
    const got = pixelsPerMeterAt(map, origin);
    expect(relErr(got.east, got.north)).toBeGreaterThan(0.005);
  });

  it('matches the drawn survey scale without a GPS origin (the 111320 m/deg display convention)', () => {
    const map = fakeMercatorMap({ lng: 0.0002, lat: 0.0003 }, 18);
    const expected = drawnPixelsPerMeter(map, null);
    const got = pixelsPerMeterAt(map, null);
    expect(relErr(got.east, expected.east)).toBeLessThan(1e-6);
    expect(relErr(got.north, expected.north)).toBeLessThan(1e-6);
  });

  it('never returns zero', () => {
    const flat = { getCenter: () => ({ lng: 0, lat: 0 }), project: () => ({ x: 0, y: 0 }) } as unknown as maplibregl.Map;
    expect(pixelsPerMeterAt(flat, null)).toEqual({ east: 1, north: 1 });
  });
});
