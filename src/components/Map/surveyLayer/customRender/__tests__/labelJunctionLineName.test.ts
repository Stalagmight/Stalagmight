/**
 * RS-FX8, N1 (shots 03, 10, 55 of the RS capture): a branch that starts on a
 * junction drew its line name on the junction, where the junction's station
 * label sits. Both labels are offset in em from one point, so they overlap at
 * every zoom; line names place first, so the junction's label never cleared,
 * was not emitted, and turning line names off could not bring it back until a
 * full rebuild.
 *
 * - A line name anchors on one of its own stations (not a junction slot, not a
 *   loop end): a branch's name sits on its first own station, a free line's
 *   on its first station as before, a one-shot tie between two lines in the
 *   middle of its shot.
 * - A label a shown line name beats is emitted hidden, so the reveal-only
 *   rerun of a line-name toggle can show it.
 *
 * Injections: anchor on the first station again (the junction's label loses
 * in the controller test); drop `shadowedByKind` from the build (the station
 * under a line name stays hidden after the toggle).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import { emptySurveyData, type SurveyData } from '../../../../../types/survey-data';
import type { MapThemeColors } from '../../../../../utils/themeColors';
import { GlyphAtlas, SDF_BUFFER, type GlyphRasterizer, type RasterizedGlyph } from '../glyphAtlas';
import { LABEL_KIND } from '../labelShaders';
import { buildLabelDescriptors, type LabelBuildContext, type LabelDescriptor } from '../labelTessellate';
import { REVEAL_CEILING, REVEAL_NEVER, shadowedByKind } from '../labelReveal';
import type { ExtentEm } from '../labelLayout';
import { SurveyLabelController, type LabelSyncParams } from '../labelController';

const h = vi.hoisted(() => {
  const state = { last: null as { updateCalls: number; revealCalls: number } | null };
  class FakeLabelLayer {
    readonly id: string;
    data: unknown;
    updateCalls = 0;
    revealCalls = 0;
    constructor(id: string, data: unknown) {
      this.id = id;
      this.data = data;
      state.last = this;
    }
    onAdd(): void {}
    isDisabled(): boolean {
      return false;
    }
    getDisabledReason(): string | null {
      return null;
    }
    update(data: unknown): void {
      this.data = data;
      this.updateCalls++;
    }
    updateFlags(): void {}
    updateReveal(): void {
      this.revealCalls++;
    }
    setStyleInputs(): void {}
  }
  return { state, FakeLabelLayer };
});
vi.mock('../surveyCustomLabelLayer', () => ({ SurveyCustomLabelLayer: h.FakeLabelLayer }));

const CTX: LabelBuildContext = {
  colorCtx: { labelColor: '#e0e0e0' } as never,
  precision: undefined,
  stationLabelText: 'id',
};

/**
 * Main (line 1): stations 0..2 at x 0, 10, 20 m (Station IDs 7, 8, 9).
 * Branch (line 2): starts on Main / 1 (the same Station ID 8, a junction),
 * then 10 m south to its own station (Station ID 5).
 * Tie (line 3, when `tie`): one shot from Main / 0 to Main / 2, both ends
 * junctions.
 */
function survey(tie = false): SurveyData {
  const d = emptySurveyData();
  d.lines = [
    { id: 1, name: 'Main', color: '#ff0000', direction: 'in' as never, quality: 0, next_shot_id: 2, shotOffset: 0, shotCount: 2 },
    { id: 2, name: 'Branch', color: '#00ff00', direction: 'in' as never, quality: 0, next_shot_id: 1, shotOffset: 2, shotCount: 1 },
    ...(tie
      ? [{ id: 3, name: 'Tie', color: '#0000ff', direction: 'in' as never, quality: 0, next_shot_id: 1, shotOffset: 3, shotCount: 1 }]
      : []),
  ];
  d.shotCount = tie ? 4 : 3;
  d.shotIds = new Uint32Array(tie ? [0, 1, 0, 0] : [0, 1, 0]);
  d.shotLineIds = new Uint32Array(tie ? [1, 1, 2, 3] : [1, 1, 2]);
  d.shotFlags = new Uint8Array(d.shotCount);
  d.stationCount = tie ? 7 : 5;
  d.stationLineIds = new Uint32Array(tie ? [1, 1, 1, 2, 2, 3, 3] : [1, 1, 1, 2, 2]);
  d.stationIndices = new Uint32Array(tie ? [0, 1, 2, 0, 1, 0, 1] : [0, 1, 2, 0, 1]);
  d.stationIds = new Uint32Array(tie ? [7, 8, 9, 8, 5, 7, 9] : [7, 8, 9, 8, 5]);
  const coords = [0, 0, 0, 10, 0, 0, 20, 0, 0, 10, 0, 0, 10, -10, 0];
  if (tie) coords.push(0, 0, 0, 20, 0, 0);
  d.stationCoords = new Float64Array(coords);
  d.stationFlags = new Uint8Array(d.stationCount);
  d.shotComments = new Map();
  d.stationComments = new Map();
  return d;
}

const lineName = (descs: LabelDescriptor[], id: number) =>
  descs.find((x) => x.kind === LABEL_KIND.LINE_NAME && x.lineId === id)!;
const stationAt = (descs: LabelDescriptor[], g: number) =>
  descs.find((x) => x.kind === LABEL_KIND.STATION_LABEL && x.stationGlobal === g)!;

describe('a line name anchors on an own station (N1)', () => {
  it("puts a branch's name on its first own station, not on the junction", () => {
    const d = survey();
    const descs = buildLabelDescriptors(d, null, 0, 0, CTX);
    const branch = lineName(descs, 2);
    const own = stationAt(descs, 4);
    expect([branch.anchorX, branch.anchorY]).toEqual([own.anchorX, own.anchorY]);
    // The junction's label (Main / 1 represents it) keeps its own point.
    const junction = stationAt(descs, 1);
    expect([junction.anchorX, junction.anchorY]).not.toEqual([branch.anchorX, branch.anchorY]);
    // A line whose first station is its own keeps it.
    const main = lineName(descs, 1);
    const first = stationAt(descs, 0);
    expect([main.anchorX, main.anchorY]).toEqual([first.anchorX, first.anchorY]);
  });

  it('puts a one-shot tie between two lines in the middle of its shot', () => {
    const d = survey(true);
    const descs = buildLabelDescriptors(d, null, 0, 0, CTX);
    const tie = lineName(descs, 3);
    const a = stationAt(descs, 0);
    const b = descs.find((x) => x.kind === LABEL_KIND.STATION_LABEL && x.stationGlobal === 2)!;
    expect(tie.anchorX).toBeCloseTo((a.anchorX + b.anchorX) / 2, 12);
    expect(tie.anchorY).toBeCloseTo((a.anchorY + b.anchorY) / 2, 12);
  });

  it('avoids a loop end, whose kept end carries the label', () => {
    const d = survey();
    // Branch's start is a loop end now (no junction id): the name still moves on.
    d.stationIds = new Uint32Array([7, 8, 9, 3, 5]);
    d.stationShown = new Map([[3, { id: 8, home_line: 1, foreign: true, loop_end: true }]]) as never;
    const descs = buildLabelDescriptors(d, null, 0, 0, CTX);
    const branch = lineName(descs, 2);
    const own = stationAt(descs, 4);
    expect([branch.anchorX, branch.anchorY]).toEqual([own.anchorX, own.anchorY]);
  });
});

describe('shadowedByKind', () => {
  const extent: ExtentEm = { minX: 0, maxX: 2, minY: 0, maxY: 1 };
  const at = (kind: number, x: number): LabelDescriptor =>
    ({ kind, text: 'x', anchorX: x, anchorY: 0, color: [0, 0, 0, 255], ordinal: 0, flags: 0 }) as unknown as LabelDescriptor;

  it('lists the dropped labels under a label of the kind, and nothing else', () => {
    const descs = [at(LABEL_KIND.LINE_NAME, 0), at(LABEL_KIND.STATION_LABEL, 0), at(LABEL_KIND.STATION_LABEL, 1), at(LABEL_KIND.STATION_LABEL, 0)];
    // Station 1 is dropped and under the name; station 2 is far away (1 unit x
    // 1000 px); station 3 sits under the name but cleared (finite reveal).
    const reveal = [10, REVEAL_NEVER, REVEAL_NEVER, 15];
    const out = shadowedByKind(descs, [extent, extent, extent, extent], reveal, LABEL_KIND.LINE_NAME, 18, 1000);
    expect([...out]).toEqual([1]);
  });
});

const THEME = { textHalo: '#111111', textHaloAlt: '#222222', depthShallow: '#00bfff', depthDeep: '#ff4444', labelColor: '#eeeeee' } as unknown as MapThemeColors;

/** A fake map with bounds, so the reveal pass runs (see labelController.test). */
function cameraMap(zoom: number): MaplibreMap {
  const center = 10 / 111320;
  const map = {
    getStyle: () => ({}),
    getLayer: () => undefined,
    addLayer: (layer: { onAdd?: (m: unknown, gl: unknown) => void }) => layer.onAdd?.(map, {}),
    removeLayer: () => {},
    setLayoutProperty: () => {},
    triggerRepaint: () => {},
    getZoom: () => zoom,
    isMoving: () => false,
    getBounds: () => {
      const wps = 512 * 2 ** zoom;
      const lngHalf = (360 * 800) / wps / 2;
      const latHalf = (360 * 600) / wps / 2;
      return {
        getWest: () => center - lngHalf,
        getEast: () => center + lngHalf,
        getNorth: () => center + latHalf,
        getSouth: () => center - latHalf,
      };
    },
    getCanvas: () => ({ clientWidth: 800, clientHeight: 600, width: 800, height: 600 }),
    on: () => {},
    off: () => {},
  } as unknown as MaplibreMap;
  return map;
}

function atlas(): GlyphAtlas {
  const raster: GlyphRasterizer = (c: string): RasterizedGlyph => {
    const empty = c === ' ';
    const w = empty ? 0 : 10;
    const ht = empty ? 0 : 18;
    return {
      data: new Uint8Array(Math.max(0, w * ht)),
      width: w,
      height: ht,
      glyphWidth: Math.max(0, w - 2 * SDF_BUFFER),
      glyphHeight: Math.max(0, ht - 2 * SDF_BUFFER),
      glyphTop: 15,
      glyphLeft: 1,
      glyphAdvance: empty ? 6 : 12,
    };
  };
  return new GlyphAtlas({ rasterizer: raster });
}

describe('the junction keeps its label beside a branch name (N1)', () => {
  beforeEach(() => {
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  const params = (): LabelSyncParams => ({
    surveyData: survey(),
    gpsOrigin: null,
    themeColors: THEME,
    showDepthGradient: false,
    depthRange: { min: 0, max: 1 },
    precision: undefined,
  });
  const revealOf = (c: SurveyLabelController, text: string) =>
    c.snapshot().find((l) => l.kind === LABEL_KIND.STATION_LABEL && l.text === text)?.revealZoom;

  it('draws the junction label with line names on, and a toggle shows the station a name covered', () => {
    const c = new SurveyLabelController(cameraMap(18), undefined, { atlas: atlas() });
    c.setStationLabelMode('id');
    c.sync(params());
    const layer = h.state.last!;
    // The junction (Station ID 8) is labelled while the branch name is shown.
    expect(revealOf(c, '8')!).toBeLessThanOrEqual(REVEAL_CEILING);
    // Main's first station (7) sits under Main's name: emitted, hidden.
    expect(revealOf(c, '7')).toBeGreaterThan(REVEAL_CEILING);

    const updates = layer.updateCalls;
    c.setLineNamesVisible(false);
    // No rebuild: the reveal-only rerun shows the covered station.
    expect(layer.updateCalls).toBe(updates);
    expect(revealOf(c, '7')!).toBeLessThanOrEqual(REVEAL_CEILING);
    expect(revealOf(c, '5')!).toBeLessThanOrEqual(REVEAL_CEILING);
    // And back: the names win their spots again.
    c.setLineNamesVisible(true);
    expect(revealOf(c, '7')).toBeGreaterThan(REVEAL_CEILING);
    expect(revealOf(c, '8')!).toBeLessThanOrEqual(REVEAL_CEILING);
  });
});
