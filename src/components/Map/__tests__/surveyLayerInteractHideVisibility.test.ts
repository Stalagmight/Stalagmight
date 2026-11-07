/**
 * The Stations / Shots toggles against a pending interact-overlay hide.
 *
 * A dim-set change (or a removal flush) hides the four interact-overlay layers
 * until the interact source re-tiles, with a 2 s fallback
 * (interactOverlayHide.ts). The hide captures each layer's visibility when it
 * starts and puts it back when it ends. The interact station layer follows the
 * Stations toggle and the two interact shot layers follow the Shots toggle
 * (SurveyLayer.applyStockVisibility), so a toggle the user flips while the hide
 * is pending must still hold once it ends: the restore may not put back the
 * visibility it captured before the toggle.
 *
 * Drives a REAL SurveyLayer over the fake map (surveyLayerFakeMap.ts).
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
import { makeSection, makeShot, makeStation } from '../../../test/factories/survey';
import type { Section, Station } from '../../../types/project';

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

const STATIONS = AUX_IDS.interactStationsLayerId;
const SHOTS = [AUX_IDS.interactShotsLayerId, AUX_IDS.interactShotsMeasuredLayerId];
const OVERLAY_LAYERS = [...SHOTS, STATIONS, AUX_IDS.stationsHighlightLayerId];

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

/** The interact source finishes its re-tile: the hide's normal end. */
function retileDone(): void {
  fake.fire('sourcedata', { sourceId: AUX_IDS.interactSourceId, isSourceLoaded: true });
}

/** The hide never sees its re-tile: the fallback timer ends it. */
function fallbackDone(): void {
  vi.advanceTimersByTime(INTERACT_HIDE_FALLBACK_MS + 1);
}

function report(label: string, ids: readonly string[], expected: string): void {
  for (const id of ids) console.log(`[${label}] ${id} visibility: ${visibility(id)} (expected ${expected})`);
}

describe('a Stations / Shots toggle during an interact-overlay hide (#576 W38-NF-1)', () => {
  for (const [end, settle] of [['re-tile', retileDone], ['fallback', fallbackDone]] as const) {
    it(`Stations turned off during the hide stay off after its ${end} restore`, () => {
      armHide();
      layer.setShowStations(false);
      settle();

      report(`stations off, ${end}`, [STATIONS], 'none');
      expect(visibility(STATIONS)).toBe('none');
      // The layers the toggle does not own come back.
      for (const id of [...SHOTS, AUX_IDS.stationsHighlightLayerId]) expect(visibility(id), id).toBe('visible');
    });

    it(`Shots turned off during the hide stay off after its ${end} restore`, () => {
      armHide();
      layer.setShowShots(false);
      settle();

      report(`shots off, ${end}`, SHOTS, 'none');
      for (const id of SHOTS) expect(visibility(id), id).toBe('none');
      for (const id of [STATIONS, AUX_IDS.stationsHighlightLayerId]) expect(visibility(id), id).toBe('visible');
    });
  }

  it('Stations hidden before the hide and turned on during it come back on', () => {
    layer.setShowStations(false);
    armHide();
    layer.setShowStations(true);
    retileDone();

    report('stations on', [STATIONS], 'visible');
    for (const id of OVERLAY_LAYERS) expect(visibility(id), id).toBe('visible');
  });

  it('control: with no toggle during the hide, its restore brings every layer back', () => {
    armHide();
    retileDone();
    for (const id of OVERLAY_LAYERS) expect(visibility(id), id).toBe('visible');
  });
});

/**
 * A context restore replays the flush the loss swallowed. When that flush removes
 * the section of the selected station (a delete or undo during the loss), the
 * flush itself hides the overlay across the interact source's re-tile; the rest
 * of the replay re-applies the Stations / Shots visibility and must not end that
 * hide early.
 */
describe("a hide armed by the context-restore replay's own flush (#576 W38-NF-2)", () => {
  /** Section `id` with one shot, its two stations `x` metres east of the origin. */
  function sectionAt(id: number, x: number): { section: Section; stations: Station[] } {
    return {
      section: makeSection({ id, name: `SectionA${id}`, shots: [makeShot({ id: 1, azimuth_in: 90, length: 5 })] }),
      stations: [0, 1].map((i) =>
        makeStation({ station_ref: { section_id: id, station_index: i }, coordinates: { x: x + i * 5, y: 0, z: 0 } }),
      ),
    };
  }

  /** `update()` and its coalesced microtask flush. */
  async function publish(parts: { section: Section; stations: Station[] }[]): Promise<void> {
    layer.update(parts.map((p) => p.section), parts.flatMap((p) => p.stations));
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
  }

  beforeEach(() => {
    // No WebGL under node: the custom geometry and label builds are not run.
    const targets = layer as unknown as { customRender: { sync: () => void }; customLabels: { sync: () => void } };
    vi.spyOn(targets.customRender, 'sync').mockImplementation(() => {});
    vi.spyOn(targets.customLabels, 'sync').mockImplementation(() => {});
  });

  afterEach(() => layer.release());

  it('the overlay stays hidden after the restore until the re-tile, then comes back', async () => {
    const a = sectionAt(7, 0);
    const b = sectionAt(8, 20);
    await publish([a, b]);
    layer.setSelectedStation({ section_id: 7, station_index: 0 });
    for (const id of OVERLAY_LAYERS) expect(visibility(id), id).toBe('visible');

    // The loss as MapLibre delivers it, then section 7 deleted while it lasts.
    fake.container.dispatchEvent(new Event('webglcontextlost'));
    fake.loseStyle();
    fake.fire('webglcontextlost');
    await publish([b]);
    // The restore: the serialized style comes back and loads; the replay flush
    // drops section 7, whose station the overlay was highlighting.
    fake.restoreStyle();
    fake.fire('webglcontextrestored');
    fake.fire('style.load');

    report('after restore', OVERLAY_LAYERS, 'none');
    for (const id of OVERLAY_LAYERS) expect(visibility(id), id).toBe('none');

    retileDone();
    report('after re-tile', OVERLAY_LAYERS, 'visible');
    for (const id of OVERLAY_LAYERS) expect(visibility(id), id).toBe('visible');
  });

  it('a toggle re-applied during a hide does not end it early for the other layers', () => {
    armHide();
    layer.setShowStations(false);

    report('stations toggled during the hide', OVERLAY_LAYERS, 'none');
    for (const id of OVERLAY_LAYERS) expect(visibility(id), id).toBe('none');
  });
});
