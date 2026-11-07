/**
 * The frame between a style rebuild's `webglcontextrestored` and its `style.load`
 * (#576 RV18-MAP-1). MapLibre 6 rebuilds the style on a WebGL context restore and
 * on a worker restart with `setStyle`, which puts a NEW style in place that loads
 * one frame later. Until then that style is empty and refuses every write with
 * "Style is not done loading.", so a setter that only checked for a style threw
 * there - out of a React effect, which takes the map panel down. The setters skip
 * the write in that frame, and the restore replay puts it on the loaded style.
 *
 * Drives a REAL SurveyLayer over the fake map (surveyLayerFakeMap.ts, whose
 * `restoreStyleLoading` / `finishStyleLoad` model that frame).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// maplibre-gl sets up its worker at module init, which node lacks; SurveyLayer
// only needs the type surface here.
vi.mock('maplibre-gl', () => ({ default: {} }));

import SurveyLayer from '../SurveyLayer';
import { AUX_IDS } from '../surveyLayer/auxiliaryLayers';
import type { SurveyGeometryIndex } from '../surveyLayer/geometryIndex';
import { makeFakeSurveyMap, type FakeSurveyMap } from './surveyLayerFakeMap';

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

/** A WebGL context loss and restore, stopped in the frame before the rebuilt style loads. */
function contextRestoreLoading(): void {
  fake.container.dispatchEvent(new Event('webglcontextlost'));
  fake.loseStyle();
  fake.fire('webglcontextlost');
  fake.restoreStyleLoading();
  fake.fire('webglcontextrestored');
}

/** A worker restart (`_handleWorkersRestarted`), stopped in the same frame: no
 *  canvas event, the synthetic loss while the old style is still in place. */
function workerRestartLoading(): void {
  fake.fire('webglcontextlost');
  fake.loseStyle();
  fake.restoreStyleLoading();
  fake.fire('webglcontextrestored');
}

const PATHS: Array<[string, () => void]> = [
  ['a WebGL context restore', contextRestoreLoading],
  ['a worker restart', workerRestartLoading],
];

/** Run `write` in the loading frame; report what it threw (null: nothing). */
function inLoadingFrame(enter: () => void, write: () => void): string | null {
  enter();
  expect(fake.styleLoading()).toBe(true);
  try {
    write();
    return null;
  } catch (e) {
    return (e as Error).message;
  }
}

const visibility = (id: string): unknown => fake.map.getLayoutProperty(id, 'visibility');

describe.each(PATHS)('a style write in the frame before %s has loaded the style (#576 RV18-MAP-1)', (label, enter) => {
  it('the Stations / Shots toggles do not throw, and land once the style has loaded', () => {
    const thrown = inLoadingFrame(enter, () => {
      layer.setShowStations(false);
      layer.setShowShots(false);
    });
    console.log(`[RV18-MAP-1] ${label}: visibility toggles in the loading frame threw:`, thrown);
    expect(thrown).toBeNull();

    fake.finishStyleLoad();
    expect(visibility(AUX_IDS.loopClosuresLayerId)).toBe('none');
    expect(visibility(AUX_IDS.interactStationsLayerId)).toBe('none');
    expect(visibility(AUX_IDS.interactShotsLayerId)).toBe('none');
  });

  it('the anchored dim of the loop closures does not throw, and lands once the style has loaded', () => {
    const thrown = inLoadingFrame(enter, () => layer.setAnchoredSections(new Set([7])));
    console.log(`[RV18-MAP-1] ${label}: anchored dim in the loading frame threw:`, thrown);
    expect(thrown).toBeNull();

    fake.finishStyleLoad();
    const opacity = fake.paint(AUX_IDS.loopClosuresLayerId, 'line-opacity');
    expect(Array.isArray(opacity) && opacity[0] === 'case').toBe(true);
  });
});

/** A filter of the closures layer's own, in place before any carry. */
const ORIGINAL = ['==', ['get', 'kind'], 'closure'];
const setOriginal = (): void => void fake.map.setFilter(AUX_IDS.loopClosuresLayerId, ORIGINAL as never);

describe('a segment carry across the loading frame (#576 RV18-MAP-1)', () => {
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

  it.each(PATHS)('a pick-up in the frame before %s has loaded the style does not throw', (label, enter) => {
    armSegmentCarry();
    const thrown = inLoadingFrame(enter, () => layer.beginDragPreview(new Set([7])));
    console.log(`[RV18-MAP-1] ${label}: segment pick-up in the loading frame threw:`, thrown);
    expect(thrown).toBeNull();
    fake.finishStyleLoad();
    layer.endDragPreview();
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toBeUndefined();
  });

  /** The closure exclusion a segment pick-up of `ids` puts on the closures layer. */
  const exclusionOf = (ids: number[]): unknown => ['!', ['any',
    ['in', ['get', 'sectionIdA'], ['literal', ids]],
    ['in', ['get', 'sectionIdB'], ['literal', ids]],
  ]];

  it.each(PATHS)('a pick-up in the frame before %s has loaded the style hides the carried closures once it has (#576 RV19b-M1)', (label, enter) => {
    armSegmentCarry();
    setOriginal();
    expect(inLoadingFrame(enter, () => layer.beginDragPreview(new Set([7])))).toBeNull();
    fake.finishStyleLoad();

    // Still carrying: the bulk layer must not draw the carried closures at their old place.
    const during = fake.filter(AUX_IDS.loopClosuresLayerId);
    console.log(`[RV19b-M1] ${label}: closure filter during the carry, after the style loaded:`, JSON.stringify(during ?? null));
    expect(during).toEqual(['all', ORIGINAL, exclusionOf([7])]);

    layer.endDragPreview();
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toEqual(ORIGINAL);
  });

  it('a pick-up during a loss that cancelled an earlier carry hides only its own closures once the style has loaded (#576 RV19b-M1)', () => {
    armSegmentCarry();
    setOriginal();
    layer.beginDragPreview(new Set([7]));
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toEqual(['all', ORIGINAL, exclusionOf([7])]);

    fake.container.dispatchEvent(new Event('webglcontextlost'));
    fake.loseStyle();
    fake.fire('webglcontextlost');
    layer.endDragPreview(); // the first carry is cancelled during the loss
    layer.beginDragPreview(new Set([8])); // and the next one picked up in it
    fake.restoreStyleLoading();
    fake.fire('webglcontextrestored');
    fake.finishStyleLoad();

    // The restored style came back with the first carry's exclusion serialized in;
    // the second carry's goes onto the real original instead.
    const during = fake.filter(AUX_IDS.loopClosuresLayerId);
    console.log('[RV19b-M1] pick-up during the loss, after the style loaded:', JSON.stringify(during ?? null));
    expect(during).toEqual(['all', ORIGINAL, exclusionOf([8])]);

    layer.endDragPreview();
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toEqual(ORIGINAL);
  });

  it.each(PATHS)('a carry cancelled in the frame before %s has loaded the style puts the closures back', (label, enter) => {
    armSegmentCarry();
    layer.beginDragPreview(new Set([7]));
    // The pick-up hides the closures touching the carried section; the rebuilt
    // style is serialized with that exclusion in it.
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toBeTruthy();

    const thrown = inLoadingFrame(enter, () => layer.endDragPreview());
    expect(thrown).toBeNull();
    fake.finishStyleLoad();

    const left = fake.filter(AUX_IDS.loopClosuresLayerId);
    console.log(`[RV18-MAP-1] ${label}: closure filter after a cancel in the loading frame:`, JSON.stringify(left ?? null));
    expect(left).toBeUndefined();
  });
});

describe('a station carry across the loading frame (#576 RV19b-M1)', () => {
  it.each(PATHS)('a pick-up in the frame before %s has loaded the style hides its closures once it has', (label, enter) => {
    // Station 7:0 alone, with one (unaligned) closure to 8:0: the carry stretches it.
    Object.assign(layer as unknown as Record<string, unknown>, {
      geoIndex: {
        section: (id: number) => ({ id, color: '#ffffff' }),
        sectionGeometry: () => ({ stations: [], shots: [] }),
        stationCoords: () => [0, 0],
      } as unknown as SurveyGeometryIndex,
      cachedSurveyData: {
        stationCount: 2,
        stationSectionIds: [7, 8],
        stationIndices: [0, 0],
        stationCoords: [0, 0, 0, 10, 0, 0],
        stationFlags: [0, 0],
        stationMergedWith: new Map(),
        stationClosuresWith: new Map(),
      },
      cachedLoopClosures: [{
        id: 1,
        station_a: { section_id: 7, station_index: 0 },
        station_b: { section_id: 8, station_index: 0 },
        is_aligned: false,
        error_percentage: 1,
      }],
    });
    setOriginal();
    expect(inLoadingFrame(enter, () => layer.beginStationDragPreview(7, 0))).toBeNull();
    fake.finishStyleLoad();

    const during = fake.filter(AUX_IDS.loopClosuresLayerId);
    console.log(`[RV19b-M1] ${label}: closure filter during the station carry, after the style loaded:`, JSON.stringify(during ?? null));
    expect(during).toEqual(['all', ORIGINAL, ['!', ['any',
      ['all', ['==', ['get', 'stationA'], '7:0'], ['==', ['get', 'stationB'], '8:0']],
    ]]]);

    layer.endDragPreview();
    expect(fake.filter(AUX_IDS.loopClosuresLayerId)).toEqual(ORIGINAL);
  });
});
