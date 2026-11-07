/**
 * The interact-overlay hide across a MapLibre worker restart.
 *
 * A dim-set change (or a removal flush) hides the four interact-overlay layers -
 * the snap ring `survey-stations-highlight` among them - until the interact
 * source re-tiles, with a 2 s fallback (interactOverlayHide.ts). A worker restart
 * (`Map._handleWorkersRestarted`) serializes the style FIRST, so a hide pending
 * then is baked into the rebuilt copy as `visibility: none`; the copy is put in
 * place empty and loads one frame later (`style.load`). SurveyLayer's restore of
 * the hide runs on that `style.load` (glContextRecovery's `beforeLoss`).
 *
 * When the hide's fallback fires inside that loading frame, its restore finds no
 * layer on the empty style. The rebuilt style must still come back with every
 * overlay layer visible once it has loaded: the replay re-derives the interact
 * shot / station layers from the Stations / Shots toggles, but nothing else
 * writes the snap ring's visibility.
 *
 * Drives a REAL SurveyLayer over the fake map (surveyLayerFakeMap.ts, whose
 * `restoreStyleLoading` / `finishStyleLoad` model the loading frame).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// maplibre-gl sets up its worker at module init, which node lacks; SurveyLayer
// only needs the type surface here.
vi.mock('maplibre-gl', () => ({ default: {} }));

import SurveyLayer from '../SurveyLayer';
import { AUX_IDS } from '../surveyLayer/auxiliaryLayers';
import { INTERACT_HIDE_FALLBACK_MS } from '../surveyLayer/interactOverlayHide';
import type { SurveyGeometryIndex } from '../surveyLayer/geometryIndex';
import { makeFakeSurveyMap, type FakeSurveyMap } from './surveyLayerFakeMap';

let fake: FakeSurveyMap;
let layer: SurveyLayer;

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(() => cb(0), 16));
  vi.stubGlobal('cancelAnimationFrame', (id: ReturnType<typeof setTimeout>) => clearTimeout(id));
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  fake = makeFakeSurveyMap();
  layer = new SurveyLayer(fake.map);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const OVERLAY_LAYERS = [
  AUX_IDS.interactShotsLayerId,
  AUX_IDS.interactShotsMeasuredLayerId,
  AUX_IDS.interactStationsLayerId,
  AUX_IDS.stationsHighlightLayerId,
];

/** Effective visibility (unset is MapLibre's default, visible). */
const visibility = (id: string): string =>
  (fake.map.getLayoutProperty(id, 'visibility') as string | undefined) ?? 'visible';

/** One drawn station, 7:0, for the interact overlay to highlight. */
function stubStation(): void {
  Object.assign(layer as unknown as Record<string, unknown>, {
    geoIndex: {
      stationCoords: () => [0, 0],
      stationGlobalIndex: () => 0,
      isStationDrawn: () => true,
      section: () => ({ color: '#ffffff' }),
    } as unknown as SurveyGeometryIndex,
    cachedSurveyData: { stationFlags: new Uint8Array(1), stationCoords: new Float64Array(3) },
  });
}

/** The app path that arms the hide: a selected station whose section Move
 *  Segment then anchors - the highlight's opacity changes, so the overlay hides
 *  across its re-tile (SurveyLayer.applyDimChangeToOverlay). */
function armHide(): void {
  stubStation();
  layer.setSelectedStation({ section_id: 7, station_index: 0 });
  layer.setAnchoredSections(new Set([7]));
  for (const id of OVERLAY_LAYERS) expect(visibility(id)).toBe('none');
}

/** `_handleWorkersRestarted` up to the loading frame: the synthetic loss while
 *  the old style is in place (serialized as it is), the copy put in place empty,
 *  the synthetic restore. */
function workerRestartLoading(): void {
  fake.fire('webglcontextlost');
  fake.loseStyle();
  fake.restoreStyleLoading();
  fake.fire('webglcontextrestored');
  expect(fake.styleLoading()).toBe(true);
}

function report(label: string): void {
  for (const id of OVERLAY_LAYERS) console.log(`[${label}] ${id} visibility: ${visibility(id)} (expected visible)`);
}

describe('the interact-overlay hide across a worker restart', () => {
  it('a fallback restore that lands in the loading frame still leaves the snap ring visible', () => {
    armHide();
    // The restart comes just before the hide's fallback is due...
    vi.advanceTimersByTime(INTERACT_HIDE_FALLBACK_MS - 5);
    workerRestartLoading();
    // ...which fires while the rebuilt style is still loading.
    vi.advanceTimersByTime(10);
    expect(fake.styleLoading()).toBe(true);
    fake.finishStyleLoad();
    // Let anything still pending (a re-armed fallback included) settle.
    vi.advanceTimersByTime(INTERACT_HIDE_FALLBACK_MS * 2);

    report('fallback in loading frame');
    for (const id of OVERLAY_LAYERS) expect(visibility(id), id).toBe('visible');
  });

  it('control: a hide still pending at style.load is restored by the restart itself', () => {
    armHide();
    workerRestartLoading();
    fake.finishStyleLoad();
    vi.advanceTimersByTime(INTERACT_HIDE_FALLBACK_MS * 2);

    report('pending at style.load');
    for (const id of OVERLAY_LAYERS) expect(visibility(id), id).toBe('visible');
  });
});
