/**
 * Regression test for the hit-test camera cache key (#customRender). The key
 * feeds the geometry index's projected-AABB cache: it MUST change on any camera
 * move, including a sub-pixel center shift. An earlier commit quantized the
 * center to pixel bins as a perf micro-optimization; two centers in one bin can
 * differ by ~1px (Web Mercator amplifies the latitude error away from the
 * equator), exceeding the probe's 0.5px margin, so a feature near the hit-radius
 * boundary could miss hover/click until the camera crossed a bin.
 * The quantization was reverted; this pins the exact-center behavior.
 *
 * `cameraSnapshot` (surveyLayer/surveyHitTester.ts) reads only the map, so a
 * fake map exercises it without the heavy SurveyLayer constructor.
 */
import { describe, expect, it, vi } from 'vitest';

// maplibre-gl runs `window.URL.createObjectURL` at module-init (worker setup),
// which jsdom lacks. The unit under test (cameraSnapshot) never touches
// maplibre, so stub the module in case anything on its import chain loads it.
vi.mock('maplibre-gl', () => ({ default: {} }));

import type { Map as MaplibreMap } from 'maplibre-gl';
import { cameraSnapshot } from '../surveyLayer/surveyHitTester';

interface FakeMapState {
  center: { lng: number; lat: number };
  zoom: number;
  bearing: number;
  pitch: number;
  width: number;
  height: number;
}

function snapshotFor(state: FakeMapState): { key: string; axisAligned: boolean } {
  const map = {
    getCenter: () => state.center,
    getCanvas: () => ({ width: state.width, height: state.height }),
    getBearing: () => state.bearing,
    getPitch: () => state.pitch,
    getZoom: () => state.zoom,
  };
  // cameraSnapshot() only touches these five map getters.
  return cameraSnapshot(map as unknown as MaplibreMap);
}

const base: FakeMapState = {
  center: { lng: 12.5, lat: 55.5 },
  zoom: 12,
  bearing: 0,
  pitch: 0,
  width: 800,
  height: 600,
};

describe('SurveyHitTester cameraSnapshot key', () => {
  it('changes on a sub-pixel center shift (no quantization binning)', () => {
    const a = snapshotFor(base);
    // A shift far below one device pixel at z12 - the old pixel-bin key would
    // have collapsed this into the same bin and returned an identical key.
    const b = snapshotFor({ ...base, center: { lng: 12.5 + 1e-9, lat: 55.5 + 1e-9 } });
    expect(b.key).not.toBe(a.key);
  });

  it('is stable for an identical camera', () => {
    expect(snapshotFor(base).key).toBe(snapshotFor(base).key);
  });

  it('changes on zoom / bearing / pitch / canvas size and tracks axisAligned', () => {
    const a = snapshotFor(base);
    expect(a.axisAligned).toBe(true);
    expect(snapshotFor({ ...base, zoom: 12.0001 }).key).not.toBe(a.key);
    expect(snapshotFor({ ...base, width: 801 }).key).not.toBe(a.key);
    const rotated = snapshotFor({ ...base, bearing: 30 });
    expect(rotated.key).not.toBe(a.key);
    expect(rotated.axisAligned).toBe(false); // bearing/pitch disables the fast path
    expect(snapshotFor({ ...base, pitch: 20 }).axisAligned).toBe(false);
  });
});
