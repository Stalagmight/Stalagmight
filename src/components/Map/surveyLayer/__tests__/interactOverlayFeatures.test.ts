/**
 * Gas-dim opacity clamp on the interact/hover overlay (Codex review on #397).
 *
 * `shotOverlayFeature` draws the hovered/selected copy of a shot through
 * MapLibre rather than the custom shader; #395's plan-idle/gas-dim tint set its
 * opacity to a flat `GAS_DIM_ALPHA` (0.45), which BRIGHTENS a reference leg
 * (base 0.28) instead of dimming it. The fix clamps via
 * `Math.min(GAS_DIM_ALPHA, SHOT_ALPHA_BY_KIND[kind])`, mirroring the shader's
 * `min()` clamp in shaders.ts.
 */
import { describe, expect, it } from 'vitest';
import { shotOverlayFeature, type InteractOverlayDeps } from '../interactOverlayFeatures';
import { SurveyGeometryIndex } from '../geometryIndex';
import type { Projector } from '../dragPreviewSource';
import { surveyDataFromLegacy } from '../../../../types/survey-data';
import { getMapThemeColors } from '../../../../utils/themeColors';
import { GAS_DIM_ALPHA, SHOT_ALPHA_REFERENCE, SHOT_ALPHA_NORMAL } from '../customRender/shaders';
import type { Section } from '../../../../types/ipc/Section';
import type { Station } from '../../../../types/ipc/Station';

const stubProjector: Projector = {
  project: (lngLat) => ({ x: lngLat[0], y: lngLat[1] }),
  unproject: (point) => ({ lng: point[0], lat: point[1] }),
};

function shot(id: number, kind: 'Normal' | 'Reference') {
  return {
    id,
    azimuth_in: 0,
    azimuth_out: 0,
    length: 10,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind,
    marker_index: null,
    temperature: null,
  };
}

function fixture() {
  const sections: Section[] = [
    {
      id: 1, name: 'A', color: '#ff0000',
      direction: 'in', quality: 1, next_shot_id: 10, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [shot(101, 'Normal'), shot(102, 'Reference')],
    },
  ];
  const st = (station_index: number, x: number, y: number): Station => ({
    station_ref: { section_id: 1, station_index },
    coordinates: { x, y, z: 0 },
    is_zero_datum: false,
    gps_coordinates: null,
    merged_with: [],
    closures_with: [],
  });
  const stations: Station[] = [st(0, 0, 0), st(1, 10, 0), st(2, 20, 0)];
  return surveyDataFromLegacy(sections, stations);
}

function baseDeps(data: ReturnType<typeof fixture>, idleActive: boolean): InteractOverlayDeps {
  return {
    geoIndex: new SurveyGeometryIndex(data, null),
    cachedSurveyData: data,
    visibleKinds: { jump: true, reference: true },
    parseFeatureId: () => null,
    measuredPathIds: null,
    measuredPathStationIds: null,
    measureTintColor: '#ffffff',
    isSectionAnchored: () => false,
    isPlanIdleActive: idleActive,
    displayColorContext: {
      showDepthGradient: false,
      depthRange: { min: 0, max: 1 },
      themeColors: getMapThemeColors(),
      showIssueSeverity: false,
    },
    themeColors: getMapThemeColors(),
    distanceMode: false,
    distanceState: null,
    gasReachMode: false,
    gasReachState: null,
    dragOverlayOffset: null,
    dragMovingShots: new Map(),
    dragMovingStationKeys: new Set(),
    dragProjector: () => stubProjector,
  };
}

describe('shotOverlayFeature gas-dim opacity clamp (#395 review)', () => {
  it('a reference leg beyond gas range keeps its fainter base alpha, never brightens', () => {
    const data = fixture();
    const feature = shotOverlayFeature(baseDeps(data, true), '1:102', 2);
    expect(feature?.properties?.opacity).toBe(SHOT_ALPHA_REFERENCE);
    expect(feature?.properties?.opacity).toBeLessThan(GAS_DIM_ALPHA);
  });

  it('a normal shot still dims to GAS_DIM_ALPHA when idle-active', () => {
    const data = fixture();
    const feature = shotOverlayFeature(baseDeps(data, true), '1:101', 2);
    expect(feature?.properties?.opacity).toBe(GAS_DIM_ALPHA);
    expect(GAS_DIM_ALPHA).toBeLessThan(SHOT_ALPHA_NORMAL);
  });

  it('idle-inactive keeps the plain per-kind base', () => {
    const data = fixture();
    const feature = shotOverlayFeature(baseDeps(data, false), '1:102', 2);
    expect(feature?.properties?.opacity).toBe(SHOT_ALPHA_REFERENCE);
  });
});
