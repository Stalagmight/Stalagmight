/**
 * #576 RV4-FE-MF2: the to-scale placement preview draws a section at the drawn
 * survey's scale on BOTH axes.
 *
 * With a GPS origin the survey is drawn through ellipsoidal ENU, so in web
 * mercator a metre east and a metre north have different screen lengths (the
 * prime-vertical vs the meridional radius). One pixels-per-metre for both axes
 * put the whole difference onto one of them: measured north, the preview's east
 * legs were up to 0.67% off the survey they are placed against.
 *
 * The oracle steps 1 m east and 1 m north through the same metres-to-map
 * transform the survey is drawn with, and the preview is fed whatever
 * `pixelsPerMeterAt` produces, as MapCanvas does.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import type * as maplibregl from 'maplibre-gl';
import PlacementOverlay from '../PlacementOverlay';
import { pixelsPerMeterAt } from '../placementScale';
import { useProjectStore } from '../../../store/projectStore';
import { mapCoordsToMeters, metersToMapCoords } from '../../../utils/coordinates';
import type { Project, SectionData, ShotData } from '../../../types/project';

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

/** The drawn survey's screen length of a 1 m step (east or north) at the map center. */
function drawnStep(map: maplibregl.Map, origin: { latitude: number; longitude: number } | null, axis: 'east' | 'north') {
  const c = map.getCenter();
  const here = mapCoordsToMeters(c.lng, c.lat, origin);
  const a = map.project(metersToMapCoords(here.x, here.y, origin));
  const to = axis === 'east' ? metersToMapCoords(here.x + 1, here.y, origin) : metersToMapCoords(here.x, here.y + 1, origin);
  const b = map.project(to);
  return Math.hypot(b.x - a.x, b.y - a.y);
}

function shot(azimuth: number, length: number): ShotData {
  return {
    azimuth_in: azimuth,
    azimuth_out: azimuth,
    length,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
  } as unknown as ShotData;
}

// 10 m due east, then 10 m due north (true north, no correction).
const SECTION: SectionData = {
  name: 'CaveA',
  surveyor: null,
  date: null,
  color: '#3b82f6',
  direction: null,
  quality: 1,
  shots: [shot(90, 10), shot(0, 10)],
};

const CURSOR = { x: 400, y: 300 };

beforeEach(() => {
  useProjectStore.setState({ project: { default_azimuth_reference: null } as unknown as Project });
});

describe('PlacementOverlay scale on both axes (#576 RV4-FE-MF2)', () => {
  const origins = [
    { name: 'equator', latitude: 0.5, longitude: 10 },
    { name: 'mid latitude', latitude: 45, longitude: -80 },
    { name: 'high latitude', latitude: 70, longitude: 20 },
  ];

  it.each(origins)('draws an east leg and a north leg at the survey scale ($name)', (o) => {
    const origin = { latitude: o.latitude, longitude: o.longitude };
    const map = fakeMercatorMap({ lng: o.longitude + 0.001, lat: o.latitude + 0.001 }, 18);
    const { container } = render(
      <PlacementOverlay
        sectionData={SECTION}
        alignStationIndex={0}
        cursorPosition={CURSOR}
        isSnapped={false}
        pixelsPerMeter={pixelsPerMeterAt(map, origin)}
      />,
    );
    const pts = Array.from(container.querySelectorAll<SVGCircleElement>('[data-testid="placement-preview"] > circle')).map(
      (c) => ({ x: Number(c.getAttribute('cx')), y: Number(c.getAttribute('cy')) }),
    );
    expect(pts).toHaveLength(3);
    const eastPx = Math.hypot(pts[1]!.x - pts[0]!.x, pts[1]!.y - pts[0]!.y);
    const northPx = Math.hypot(pts[2]!.x - pts[1]!.x, pts[2]!.y - pts[1]!.y);
    const wantEast = 10 * drawnStep(map, origin, 'east');
    const wantNorth = 10 * drawnStep(map, origin, 'north');
    const eastErr = Math.abs(eastPx - wantEast) / wantEast;
    const northErr = Math.abs(northPx - wantNorth) / wantNorth;
    // Printed so a failure shows the size of the error, not only that it exists.
    console.log(`[MF2] lat=${o.latitude}: east err=${(eastErr * 100).toFixed(3)}% north err=${(northErr * 100).toFixed(3)}%`);
    expect(eastErr).toBeLessThan(1e-6);
    expect(northErr).toBeLessThan(1e-6);
  });
});
