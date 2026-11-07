/**
 * What a WebGL context restore must put right that changed while the context was
 * lost (#576 NF-MAP-5 follow-ups). MapLibre restores by re-applying the style it
 * serialized at the loss - filters and GeoJSON source data included - so state a
 * setter could not write during the loss comes back STALE unless the restore
 * replay rewrites it.
 *
 * Drives a REAL SurveyLayer over the fake map (surveyLayerFakeMap.ts), with the
 * loss and restore fired the way MapLibre fires them.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// maplibre-gl sets up its worker at module init, which node lacks; SurveyLayer
// only needs the type surface here.
vi.mock('maplibre-gl', () => ({ default: {} }));

import SurveyLayer from '../SurveyLayer';
import { AUX_IDS } from '../surveyLayer/auxiliaryLayers';
import type { SurveyGeometryIndex } from '../surveyLayer/geometryIndex';
import { makeFakeSurveyMap, type FakeSurveyMap } from './surveyLayerFakeMap';
import type { CustomDimState } from '../surveyLayer/customRender/controller';
import { BEYOND_GAS_RANGE } from '../../../utils/gasReach';

let fake: FakeSurveyMap;
let layer: SurveyLayer;

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  fake = makeFakeSurveyMap();
  layer = new SurveyLayer(fake.map);
});

afterEach(() => {
  vi.restoreAllMocks();
});

/** The loss as MapLibre delivers it: the container snapshot, the style dropped, the map event. */
function loseContext(): void {
  fake.container.dispatchEvent(new Event('webglcontextlost'));
  fake.loseStyle();
  fake.fire('webglcontextlost');
}

/** The restore: the serialized style comes back and loads on the next frame. */
function restoreContext(): void {
  fake.restoreStyle();
  fake.fire('webglcontextrestored');
  fake.fire('style.load');
}

describe('a segment carry cancelled while the context is lost (#576 RV4-FE-SL1)', () => {
  /** Enough survey state for a segment pick-up of section 7 (no geometry to capture). */
  function armSegmentCarry(): void {
    Object.assign(layer as unknown as Record<string, unknown>, {
      geoIndex: {
        sectionGeometry: () => ({ stations: [], shots: [] }),
        stationCoords: () => null,
      } as unknown as SurveyGeometryIndex,
      cachedSurveyData: {},
      cachedSections: [{ id: 7, color: '#ffffff', shots: [] }],
    });
  }

  it("restores the loop closures' filter once the context is back", () => {
    armSegmentCarry();
    layer.beginDragPreview(new Set([7]));
    // The pick-up hides the closures touching the carried section.
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toBeTruthy();

    loseContext();
    layer.endDragPreview(); // Escape / right-click during the loss
    restoreContext();

    console.log('[SL1] closure filter after cancel-during-loss + restore:', JSON.stringify(fake.filter(AUX_IDS.loopClosuresLayerId) ?? null));
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toBeUndefined();

    // A later carry saves and restores the real (unfiltered) state, not a stale exclusion.
    layer.beginDragPreview(new Set([7]));
    layer.endDragPreview();
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toBeUndefined();
  });

  it('a carry begun during the loss and ended after the restore still restores the original filter', () => {
    armSegmentCarry();
    layer.beginDragPreview(new Set([7]));
    loseContext();
    layer.endDragPreview();
    // Picked up again while the context is still lost: no style to save from.
    layer.beginDragPreview(new Set([7]));
    restoreContext();
    // Still carrying: the exclusion may stand until the drop...
    layer.endDragPreview();
    // ...which brings the closures back.
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toBeUndefined();
  });
});

describe('a gas route cleared while the context is lost (#576 RV4-FE-SL3)', () => {
  const GAS_ROUTE_SOURCE = AUX_IDS.sourceId + '-gas-route';
  const featureCount = (): number | null => {
    const data = fake.sourceData(GAS_ROUTE_SOURCE) as { features?: unknown[] } | undefined;
    return data?.features?.length ?? null;
  };

  it('is erased by the restore instead of coming back with the serialized style', () => {
    const at = (section_id: number, station_index: number, x: number, y: number) => ({
      station_ref: { section_id, station_index },
      coordinates: { x, y, z: 0 },
    });
    Object.assign(layer as unknown as Record<string, unknown>, {
      cachedStations: [at(1, 0, 0, 0), at(1, 1, 10, 0)],
    });
    layer.setGasRoute([{ from: { section_id: 1, station_index: 0 }, to: { section_id: 1, station_index: 1 }, is_jump: false }]);
    expect(featureCount()).toBe(1);

    loseContext();
    // Leaving Plan (or the field going away) clears the route during the loss.
    layer.setGasRoute([]);
    restoreContext();

    console.log('[SL3] gas-route features after clear-during-loss + restore:', featureCount());
    expect(featureCount()).toBe(0);
  });
});

describe('gas reach turned off while the context is lost (#576 NF-W13FE-1)', () => {
  it('the restore applies the gas dim the setter could not, so no later unchanged call has to', () => {
    const beyond = new Map([[7, BEYOND_GAS_RANGE]]);
    const controller = (layer as unknown as { customRender: { setDimming: (s: CustomDimState) => void } }).customRender;
    const setDimming = vi.spyOn(controller, 'setDimming');
    layer.setGasReach(true, { shots: beyond, stations: beyond, max: 1500 });
    expect(setDimming.mock.lastCall?.[0].gasDimShotFeatureIds.has(7)).toBe(true);

    loseContext();
    // Leaving Plan during the loss: the setter stores the state but cannot apply it.
    layer.setGasReach(false, null);
    setDimming.mockClear();
    restoreContext();

    const last = setDimming.mock.lastCall?.[0];
    console.log('[NF-W13FE-1] gas-dimmed shots after off-during-loss + restore:', last ? last.gasDimShotFeatureIds.size : 'no setDimming');
    expect(last?.gasDimShotFeatureIds.size).toBe(0);
    expect(last?.gasDimStationFeatureIds.size).toBe(0);
  });
});
