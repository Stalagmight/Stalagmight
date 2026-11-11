import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import { emptySurveyData, shotKindToFlags, STATION_FLAG_IS_ZERO_DATUM, type SurveyData } from '../../../../../types/survey-data';
import type { MapThemeColors } from '../../../../../utils/themeColors';
import { GlyphAtlas, SDF_BUFFER, type GlyphRasterizer, type RasterizedGlyph } from '../glyphAtlas';
import { LABEL_KIND, LABEL_FLAG } from '../labelShaders';
import { FEATURE_ID_STRIDE } from '../../../../../utils/ids';
import type { LabelRenderData } from '../surveyCustomLabelLayer';
import { SurveyLabelController, type LabelSyncParams } from '../labelController';
import { CUSTOM_LABELS_SYNC_HOOK } from '../labelsGate';
import { REVEAL_CEILING } from '../labelReveal';
import { computeMercatorAnchor } from '../mercator';
import { projectStationsRel } from '../tessellate';

/** A minimal truthy rasterizer (one 10x18 quad per non-space char) so the
 *  font-readiness happy path resolves a USABLE atlas in the node env, where the
 *  production canRasterize() would otherwise report no canvas. */
const trivialRasterizer: GlyphRasterizer = (ch: string): RasterizedGlyph => {
  const empty = ch === ' ';
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

/**
 * Label controller (#customRender labels). Same fake-GL idiom as
 * `controller.test.ts`: the label GL layer is mocked and a real (inert)
 * `GlyphAtlas` is injected so the pure build runs with no font / canvas. The
 * custom label layer is the SOLE label path (no gate, no stock fallback), so this
 * covers unconditional activation, the error-state routing (onFatal), and the
 * station-toggle / theme / reference wiring into the shader-uniform inputs.
 */
const h = vi.hoisted(() => {
  const layerState = {
    disabledAfterAdd: false,
    disabledReason: 'program-link-failed' as string,
    last: null as FakeLabelLayer | null,
  };

  class FakeLabelLayer {
    readonly id: string;
    data: unknown;
    onSelfDisable: ((reason: string) => void) | null;
    private disabled = false;
    private reason: string | null = null;
    updateCalls = 0;
    styleCalls = 0;
    lastStyle: {
      kindEnabled: boolean[];
      haloColors: unknown[];
      hideReference: boolean;
      hideJump: boolean;
    } | null = null;

    constructor(id: string, data: unknown, onSelfDisable: ((reason: string) => void) | null = null) {
      this.id = id;
      this.data = data;
      this.onSelfDisable = onSelfDisable;
      layerState.last = this;
    }
    onAdd(): void {
      if (layerState.disabledAfterAdd) {
        this.disabled = true;
        this.reason = layerState.disabledReason;
      }
    }
    isDisabled(): boolean {
      return this.disabled;
    }
    getDisabledReason(): string | null {
      return this.reason;
    }
    flagsCalls = 0;
    lastFlags: Uint8Array | null = null;
    revealCalls = 0;
    lastReveal: Float32Array | null = null;
    update(data: unknown): void {
      this.data = data;
      this.updateCalls++;
    }
    updateFlags(flags: Uint8Array): void {
      this.lastFlags = flags;
      const d = this.data as { buffers?: { flags: Uint8Array } } | null;
      if (d?.buffers) d.buffers.flags = flags;
      this.flagsCalls++;
    }
    updateReveal(revealZoom: Float32Array): void {
      this.lastReveal = revealZoom;
      const d = this.data as { buffers?: { revealZoom: Float32Array } } | null;
      if (d?.buffers) d.buffers.revealZoom = revealZoom;
      this.revealCalls++;
    }
    setStyleInputs(style: {
      kindEnabled: boolean[];
      haloColors: unknown[];
      hideReference: boolean;
      hideJump: boolean;
    }): void {
      this.lastStyle = style;
      this.styleCalls++;
    }
  }

  return { layerState, FakeLabelLayer };
});

vi.mock('../surveyCustomLabelLayer', () => ({ SurveyCustomLabelLayer: h.FakeLabelLayer }));

const CUSTOM_ID = 'survey-custom-labels';
/** All five stock text layers, with the kind each maps to (SurveyLayer ids). */
const STOCK_LAYERS: Record<number, string> = {
  [LABEL_KIND.SHOT_LABEL]: 'survey-shot-labels',
  [LABEL_KIND.STATION_LABEL]: 'survey-station-labels',
  [LABEL_KIND.SECTION_NAME]: 'survey-labels',
  [LABEL_KIND.STATION_COMMENT]: 'survey-station-comments',
  [LABEL_KIND.SHOT_COMMENT]: 'survey-shot-comments',
};

interface FakeMap {
  map: MaplibreMap;
  visibility: Record<string, string>;
  removed: string[];
}

function makeFakeMap(): FakeMap {
  const existing = new Set<string>(Object.values(STOCK_LAYERS));
  // All five stock text layers start 'visible' in the fake (the controller sets
  // the real desired state); tests assert transitions from there.
  const visibility: Record<string, string> = {};
  for (const id of Object.values(STOCK_LAYERS)) visibility[id] = 'visible';
  const removed: string[] = [];
  const map = {
    getStyle: () => ({}),
    getLayer: (id: string) => (existing.has(id) ? { id } : undefined),
    addLayer: (layer: { id: string; onAdd?: (m: unknown, gl: unknown) => void }) => {
      existing.add(layer.id);
      layer.onAdd?.(map, {});
    },
    removeLayer: (id: string) => {
      existing.delete(id);
      removed.push(id);
    },
    setLayoutProperty: (id: string, prop: string, v: string) => {
      if (prop === 'visibility') visibility[id] = v;
    },
    triggerRepaint: () => {},
  } as unknown as MaplibreMap;
  return { map, visibility, removed };
}

const THEME = { textHalo: '#111111', textHaloAlt: '#222222', depthShallow: '#00bfff', depthDeep: '#ff4444', labelColor: '#eeeeee' } as unknown as MapThemeColors;

function makeFixture(): SurveyData {
  const d = emptySurveyData();
  d.sections = [
    { id: 1, name: 'A', surveyor: null, date: null, color: '#ff0000', direction: 'in' as never, quality: 0, next_shot_id: 2, shotOffset: 0, shotCount: 2 },
    { id: 2, name: 'B', surveyor: null, date: null, color: '#00ff00', direction: 'in' as never, quality: 0, next_shot_id: 1, shotOffset: 2, shotCount: 1 },
  ];
  d.shotCount = 3;
  d.shotIds = new Uint32Array([0, 1, 0]);
  d.shotSectionIds = new Uint32Array([1, 1, 2]);
  d.shotFlags = new Uint8Array([0, 0, 0]);
  d.stationCount = 4;
  d.stationSectionIds = new Uint32Array([1, 1, 1, 2]);
  d.stationIndices = new Uint32Array([0, 1, 2, 0]);
  d.stationCoords = new Float64Array([0, 0, 0, 10, 0, -1, 10, 10, -2, 100, 100, 0]);
  d.stationFlags = new Uint8Array([STATION_FLAG_IS_ZERO_DATUM, 0, 0, 0]);
  return d;
}

function makeParams(): LabelSyncParams {
  return {
    surveyData: makeFixture(),
    gpsOrigin: null,
    themeColors: THEME,
    showDepthGradient: false,
    depthRange: { min: 0, max: 1 },
    precision: undefined,
  };
}

/** Controller with an injected inert atlas (no async font load). */
function makeController(map: MaplibreMap, opts: Record<string, unknown> = {}): SurveyLabelController {
  return new SurveyLabelController(map, undefined, { atlas: new GlyphAtlas(), ...opts });
}

describe('SurveyLabelController activation (sole label path, no gate)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('activates and hides the stock station-labels layer on sync (unconditional)', () => {
    const { map } = makeFakeMap();
    const c = makeController(map);
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);
  });

  it('is unconditional: a re-sync stays active + keeps stock hidden, no re-fire', () => {
    const changes: boolean[] = [];
    const { map } = makeFakeMap();
    const c = makeController(map, { onActiveChange: (a: boolean) => changes.push(a) });
    c.sync(makeParams());
    // A second sync is a refresh, never a disable - it stays active and the stock
    // station labels stay hidden.
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);
    expect(changes).toEqual([true]); // true fires once, never re-fires
  });

  it('fires onActiveChange(true) on enable and (false) on destroy (non-fatal teardown)', () => {
    const changes: boolean[] = [];
    const { map } = makeFakeMap();
    const c = makeController(map, { onActiveChange: (a: boolean) => changes.push(a) });
    c.sync(makeParams());
    expect(changes).toEqual([true]); // enabled → drop stock label VT
    // destroy() is the sole non-fatal teardown that restores/republishes stock.
    c.destroy();
    expect(changes).toEqual([true, false]); // torn down → restore stock label VT
  });
});

describe('SurveyLabelController error state (no stock fallback)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.disabledReason = 'program-link-failed';
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('onAdd-disabled: fires onFatal, removes the dead layer, does NOT restore stock', () => {
    h.layerState.disabledAfterAdd = true;
    h.layerState.disabledReason = 'webgl2-unavailable';
    const fatals: string[] = [];
    const { map, removed } = makeFakeMap();
    const c = makeController(map, { onFatal: (r: string) => fatals.push(r) });
    c.setStationLabelMode('id');
    c.sync(makeParams());
    expect(c.isActive()).toBe(false);
    expect(removed).toContain(CUSTOM_ID);
    expect(fatals).toEqual(['webgl2-unavailable']);
    // Fatal latch: a later sync() is inert (labels stay absent - no fallback).
    c.sync(makeParams());
    expect(removed.filter((id) => id === CUSTOM_ID).length).toBe(1);
    expect(fatals).toEqual(['webgl2-unavailable']); // at most once
  });

  it('render-error self-disable fires onFatal (deferred), removes the layer, keeps stock hidden', () => {
    const holder: { fn: (() => void) | null; scheduled: number } = { fn: null, scheduled: 0 };
    const fatals: string[] = [];
    const { map, removed } = makeFakeMap();
    const c = makeController(map, {
      onFatal: (r: string) => fatals.push(r),
      scheduleRestore: (fn: () => void) => {
        holder.fn = fn;
        holder.scheduled += 1;
      },
    });
    c.setStationLabelMode('id');
    c.sync(makeParams());

    const layer = h.layerState.last;
    layer?.onSelfDisable?.('gl-error-0x502');
    // Deferred: nothing torn down yet (must not mutate style mid-render).
    expect(removed).not.toContain(CUSTOM_ID);
    expect(fatals).toEqual([]);
    // A second self-disable before the restore runs is coalesced.
    layer?.onSelfDisable?.('gl-error-0x502');
    expect(holder.scheduled).toBe(1);

    holder.fn?.();
    // Error state: dead layer removed, onFatal fired once, stock NOT restored
    // (stays hidden from the clean enable - no fallback).
    expect(removed).toEqual([CUSTOM_ID]);
    expect(fatals).toEqual(['gl-error-0x502']);
    expect(c.isActive()).toBe(false);
  });

  it('a fatal self-disable latches + does NOT republish stock (no onActiveChange(false))', () => {
    // The fatal path removes the layer WITHOUT firing onActiveChange(false):
    // firing it would drive SurveyLayer to republish the stock label VT groups,
    // exactly the fallback this stage removes. And the fatal latch makes any later
    // sync() a no-op rather than a flip-flop that re-creates the failed layer.
    const holder: { fn: (() => void) | null } = { fn: null };
    const calls: boolean[] = [];
    const fatals: string[] = [];
    const { map, removed } = makeFakeMap();
    const c = makeController(map, {
      onActiveChange: (a: boolean) => calls.push(a),
      onFatal: (r: string) => fatals.push(r),
      scheduleRestore: (fn: () => void) => { holder.fn = fn; },
    });
    c.sync(makeParams());
    expect(calls).toEqual([true]);
    h.layerState.last?.onSelfDisable?.('gl-error-0x502');
    holder.fn?.(); // deferred fatal teardown
    expect(fatals).toEqual(['gl-error-0x502']);
    expect(calls).toEqual([true]); // no false → no stock republish
    expect(removed).toEqual([CUSTOM_ID]);
    // Later sync is inert (fatal latch), never re-enabling.
    c.sync(makeParams());
    expect(removed).toEqual([CUSTOM_ID]);
    expect(c.isActive()).toBe(false);
  });

  it('stays inert after destroy: a stray late sync does not resurrect the layer', () => {
    const { map, removed } = makeFakeMap();
    const c = makeController(map);
    c.setStationLabelMode('id'); // desired visible so destroy restores stock to visible
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);
    c.destroy();
    expect(c.isActive()).toBe(false);
    const removedCount = removed.length;
    // A deferred callback (camera-settle / font-resolve) firing after teardown
    // must not re-add the layer or re-suppress the stock labels.
    c.sync(makeParams());
    expect(c.isActive()).toBe(false);
    expect(removed.length).toBe(removedCount);
  });

  it('ignores a font-resolve that lands after destroy (no build, no activate)', async () => {
    const { map } = makeFakeMap();
    let resolveFont!: (ready: boolean) => void;
    const fontPromise = new Promise<boolean>((r) => {
      resolveFont = r;
    });
    const c = new SurveyLabelController(map, undefined, {
      ensureFont: () => fontPromise,
      makeRasterizer: () => null as never,
    });
    c.sync(makeParams());
    expect(c.isActive()).toBe(false); // atlas not resolved yet
    c.destroy();
    resolveFont(true); // font resolves on a torn-down controller
    await fontPromise;
    await Promise.resolve(); // flush the .then microtask
    expect(c.isActive()).toBe(false);
  });
});

describe('SurveyLabelController style wiring', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('drives the station-label kind visibility via the style inputs (no rebuild), stock stays suppressed', () => {
    const { map } = makeFakeMap();
    const c = makeController(map);
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const updatesBefore = layer.updateCalls;

    c.setStationLabelMode('id');
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.STATION_LABEL]).toBe(true);
    c.setStationLabelMode('off');
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.STATION_LABEL]).toBe(false);
    // A toggle is a uniform flip, not a buffer rebuild.
    expect(layer.updateCalls).toBe(updatesBefore);
    // Stock station labels never un-hide while active.
  });

  it('resolves theme halo colors into the style inputs on build', () => {
    const { map } = makeFakeMap();
    const c = makeController(map);
    c.sync(makeParams());
    const layer = h.layerState.last!;
    // Station labels use textHaloAlt (#222222) -> non-empty resolved halo set.
    expect(layer.lastStyle?.haloColors.length).toBeGreaterThan(0);
  });

  it('hides each virtual kind\'s labels with its own toggle (#27)', () => {
    // Regression: label flags used to carry a single "virtual" bit and the
    // controller only hid labels when BOTH kinds were off. Because the LINE
    // shader is per-kind, hiding References alone collapsed their geometry
    // while their shot-data and comment labels stayed drawn in empty space -
    // the orphan-label bug, mirrored for Jumps. The flags now carry the kind.
    const { map } = makeFakeMap();
    const c = makeController(map);
    c.sync(makeParams());
    const layer = h.layerState.last!;
    expect(layer.lastStyle?.hideReference).toBe(false);
    expect(layer.lastStyle?.hideJump).toBe(false);

    c.setReferenceShotsVisible(false);
    expect(layer.lastStyle?.hideReference).toBe(true);
    expect(layer.lastStyle?.hideJump).toBe(false);

    c.setJumpShotsVisible(false);
    expect(layer.lastStyle?.hideReference).toBe(true);
    expect(layer.lastStyle?.hideJump).toBe(true);

    c.setReferenceShotsVisible(true);
    expect(layer.lastStyle?.hideReference).toBe(false);
    expect(layer.lastStyle?.hideJump).toBe(true);
  });
});

describe('SurveyLabelController all-kinds suppression + toggle routing', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('enables the custom label layer (sole label renderer, no stock to hide)', () => {
    const { map, removed } = makeFakeMap();
    const c = makeController(map);
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);
    expect(removed).not.toContain(CUSTOM_ID);
  });

  it('routes every setShow* toggle to its kind uniform without a rebuild', () => {
    const { map } = makeFakeMap();
    const c = makeController(map);
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const updatesBefore = layer.updateCalls;

    c.setShotLabelsVisible(true);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.SHOT_LABEL]).toBe(true);
    c.setSectionNamesVisible(false);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.SECTION_NAME]).toBe(false);
    c.setStationCommentsVisible(true);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.STATION_COMMENT]).toBe(true);
    c.setShotCommentsVisible(true);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.SHOT_COMMENT]).toBe(true);
    // Errors labels: shot labels and closure labels flip independently.
    expect(layer.lastStyle?.kindEnabled).toHaveLength(7);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.SHOT_ERROR]).toBe(false);
    c.setErrorLabelsVisible(true, false);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.SHOT_ERROR]).toBe(true);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.CLOSURE_ERROR]).toBe(false);
    c.setErrorLabelsVisible(false, true);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.SHOT_ERROR]).toBe(false);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.CLOSURE_ERROR]).toBe(true);
    // Uniform flips, never a buffer rebuild.
    expect(layer.updateCalls).toBe(updatesBefore);
  });

  it('a commentTint sync builds without error (palette-tint parity locked in labelTessellate)', () => {
    const { map } = makeFakeMap();
    const c = makeController(map);
    c.sync({ ...makeParams(), commentTint: true });
    expect(c.isActive()).toBe(true);
  });
});

describe('SurveyLabelController font readiness', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => {
    vi.restoreAllMocks();
    delete (globalThis as unknown as Record<string, unknown>).OffscreenCanvas;
  });

  it('defers the first build until the font resolves (usable rasterizer), then builds + activates', async () => {
    // canRasterize() gates on OffscreenCanvas/document; the node test env has
    // neither, so stub OffscreenCanvas to model a canvas-capable runtime and pass
    // a real (truthy) rasterizer -> the resolved atlas is usable and activates.
    (globalThis as unknown as Record<string, unknown>).OffscreenCanvas = class {};
    const { map } = makeFakeMap();
    const c = new SurveyLabelController(map, undefined, {
      ensureFont: () => Promise.resolve(true),
      makeRasterizer: () => trivialRasterizer,
    });
    c.sync(makeParams());
    // Font not resolved yet: nothing built.
    expect(c.isActive()).toBe(false);
    // Flush the ensureFont().then(...) microtask chain.
    await Promise.resolve();
    await Promise.resolve();
    expect(c.isActive()).toBe(true);
  });

  it('surfaces onFatal when the bundled font is unavailable (null rasterizer, never activates)', async () => {
    // A failed font asset (or a WebView without FontFace / canvas) resolves
    // ready=false -> null rasterizer. There is no stock label fallback anymore, so
    // building an inert atlas would blank all labels silently. Instead the
    // controller must stay inactive (sync() a no-op) AND surface the failure via
    // onFatal so the app can toast, rather than dropping every label unnoticed.
    const changes: boolean[] = [];
    const fatals: string[] = [];
    const { map } = makeFakeMap();
    const c = new SurveyLabelController(map, undefined, {
      ensureFont: () => Promise.resolve(false),
      makeRasterizer: () => null,
      onActiveChange: (a: boolean) => changes.push(a),
      onFatal: (r: string) => fatals.push(r),
    });
    c.sync(makeParams());
    // Flush the ensureFont().then(...) microtask chain.
    await Promise.resolve();
    await Promise.resolve();
    expect(c.isActive()).toBe(false);
    expect(changes).toEqual([]); // never fired onActiveChange(true) -> no stock republish
    expect(fatals).toEqual(['label-font-unavailable']);
    // A later sync() stays a no-op (fatal latched); onFatal fires at most once.
    c.sync(makeParams());
    expect(c.isActive()).toBe(false);
    expect(fatals).toEqual(['label-font-unavailable']);
  });
});

describe('SurveyLabelController teardown', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('destroy() removes the layer and restores the stock label visibility', () => {
    const { map, removed } = makeFakeMap();
    const c = makeController(map);
    c.setStationLabelMode('id');
    c.sync(makeParams());
    c.destroy();
    expect(removed).toContain(CUSTOM_ID);
  });
});

describe('SurveyLabelController dispose (unmount-lite hook + cache release)', () => {
  // The controller installs its window sync hook only when `window` is defined;
  // the node test env has none, so stub a bare window for the lifetime of each
  // test and restore it afterwards.
  const realWindow = (globalThis as unknown as { window?: unknown }).window;
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    (globalThis as unknown as { window: unknown }).window = {};
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => {
    vi.restoreAllMocks();
    if (realWindow === undefined) delete (globalThis as unknown as { window?: unknown }).window;
    else (globalThis as unknown as { window: unknown }).window = realWindow;
  });

  it('dispose() removes the window sync hook and a later sync() is inert (no map touch)', () => {
    const win = (globalThis as unknown as { window: Record<string, unknown> }).window;
    const { map } = makeFakeMap();
    const c = makeController(map);
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);
    expect(typeof win[CUSTOM_LABELS_SYNC_HOOK]).toBe('function'); // hook installed
    const layer = h.layerState.last!;
    const updatesBefore = layer.updateCalls;

    c.dispose();
    expect(win[CUSTOM_LABELS_SYNC_HOOK]).toBeUndefined(); // hook removed
    // dispose() must NOT touch the map: the stock label layer stays suppressed
    // (map.remove() handles GL teardown), and a later sync() rebuilds nothing.
    c.sync(makeParams());
    expect(layer.updateCalls).toBe(updatesBefore); // inert: disposed guard bails
  });
});

// ─── Phase 4: viewport scoping + collision + camera-triggered rebuild ─────────

/** Fixed-metric fake atlas (one 10x18 quad per non-space char) so a build
 *  produces real glyphs - needed to inspect which KINDS the zoom gate built. */
function fakeGlyphAtlas(): GlyphAtlas {
  const raster: GlyphRasterizer = (c: string): RasterizedGlyph => {
    const empty = c === ' ';
    const w = empty ? 0 : 10;
    const h = empty ? 0 : 18;
    return {
      data: new Uint8Array(Math.max(0, w * h)),
      width: w,
      height: h,
      glyphWidth: Math.max(0, w - 2 * SDF_BUFFER),
      glyphHeight: Math.max(0, h - 2 * SDF_BUFFER),
      glyphTop: 15,
      glyphLeft: 1,
      glyphAdvance: empty ? 6 : 12,
    };
  };
  return new GlyphAtlas({ rasterizer: raster });
}

interface CameraMap extends FakeMap {
  state: { zoom: number; moving: boolean; cLng: number; cLat: number };
  fire: (type: string) => void;
}

/** Survey mercator center in DEGREES for the fixture (stations 0..100 m, so the
 *  center is ~50 m from the origin). The camera is centered here so the fixture
 *  is in view; `boundsForZoom` derives a physically consistent viewport. */
const FIXTURE_CENTER_DEG = 50 / 111320;

/** Fake map with camera hooks so the reveal path (zoom gate + viewport + reveal
 *  pass + conditional camera-settle rebuild) runs. Crucially, the bounds are
 *  DERIVED from `state.zoom` (world-pixel scale = 512·2^zoom, MapLibre's 512
 *  tile), so zooming in genuinely shrinks the viewport and raises the world-pixel
 *  scale - the physical premise the reveal pass relies on. `state.cLng/cLat` pan
 *  the center. */
function makeCameraMap(zoom = 18): CameraMap {
  const base = makeFakeMap();
  const state = { zoom, moving: false, cLng: FIXTURE_CENTER_DEG, cLat: FIXTURE_CENTER_DEG };
  const listeners = new Map<string, Set<() => void>>();
  const bounds = (): { west: number; east: number; north: number; south: number } => {
    const wps = 512 * 2 ** state.zoom; // px per unit mercator (mercX = lng/360 + 0.5)
    const lngHalf = ((360 * 800) / wps) / 2;
    const latHalf = ((360 * 600) / wps) / 2;
    return {
      west: state.cLng - lngHalf,
      east: state.cLng + lngHalf,
      north: state.cLat + latHalf,
      south: state.cLat - latHalf,
    };
  };
  const extra = {
    getZoom: () => state.zoom,
    isMoving: () => state.moving,
    getBounds: () => {
      const b = bounds();
      return { getWest: () => b.west, getEast: () => b.east, getNorth: () => b.north, getSouth: () => b.south };
    },
    getCanvas: () => ({ clientWidth: 800, clientHeight: 600, width: 800, height: 600 }),
    on: (type: string, fn: () => void) => {
      const set = listeners.get(type) ?? new Set<() => void>();
      set.add(fn);
      listeners.set(type, set);
    },
    off: (type: string, fn: () => void) => {
      listeners.get(type)?.delete(fn);
    },
  };
  Object.assign(base.map as unknown as Record<string, unknown>, extra);
  const fire = (type: string): void => {
    for (const fn of listeners.get(type) ?? []) fn();
  };
  return { ...base, state, fire };
}

/** Fake map with a zoom + camera-settle listeners but NO bounds/canvas, so the
 *  build runs the zoom gate (which kinds to build) but SKIPS viewport scoping +
 *  the reveal pass - every candidate label is emitted at reveal 0. Used by the
 *  flag-plumbing tests (dim / drag / toggle) that assert on specific labels'
 *  flags without collision decluttering removing any. */
function zoomOnlyMap(zoom = 18): CameraMap {
  const cam = makeCameraMap(zoom);
  delete (cam.map as unknown as Record<string, unknown>).getBounds;
  delete (cam.map as unknown as Record<string, unknown>).getCanvas;
  return cam;
}

/** The distinct label KINDS present in the last-built buffer (via a_kind). */
function builtKinds(layer: FakeLabelLayer): Set<number> {
  const data = layer.data as LabelRenderData;
  return new Set(Array.from(data.buffers.kind));
}

/** Deferred rebuild scheduler holder (drives coalescing tests). */
function rebuildHolder(): { fn: (() => void) | null; count: number; run: () => void } {
  const h2 = {
    fn: null as (() => void) | null,
    count: 0,
    run(): void {
      const f = h2.fn;
      h2.fn = null;
      f?.();
    },
  };
  return h2;
}

type FakeLabelLayer = NonNullable<typeof h.layerState.last>;

function cameraController(map: MaplibreMap, opts: Record<string, unknown> = {}): SurveyLabelController {
  return new SurveyLabelController(map, undefined, { atlas: fakeGlyphAtlas(), ...opts });
}

describe('SurveyLabelController zoom gate (viewport build set)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('builds all kinds above the build zoom (13.5)', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(makeParams());
    const kinds = builtKinds(h.layerState.last!);
    // Fixture has shot labels, station labels, section names (no comments).
    expect(kinds.has(LABEL_KIND.SHOT_LABEL)).toBe(true);
    expect(kinds.has(LABEL_KIND.STATION_LABEL)).toBe(true);
    expect(kinds.has(LABEL_KIND.SECTION_NAME)).toBe(true);
  });

  it('builds ONLY section names between section-minzoom (10) and the build band', () => {
    const cam = zoomOnlyMap(12);
    const c = cameraController(cam.map);
    c.sync(makeParams());
    expect(builtKinds(h.layerState.last!)).toEqual(new Set([LABEL_KIND.SECTION_NAME]));
  });

  it('builds nothing below section minzoom (10)', () => {
    const cam = zoomOnlyMap(8);
    const c = cameraController(cam.map);
    c.sync(makeParams());
    const data = h.layerState.last!.data as LabelRenderData;
    expect(data.buffers.glyphCount).toBe(0);
  });

  it('keeps the full label set on the way down through the hysteresis band [13, 13.5)', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(makeParams()); // build full at 18
    cam.state.zoom = 13.2; // inside the band, still >= drop (13)
    c.sync(makeParams());
    // Hysteresis: already-built stays full (shot labels present at 13.2).
    expect(builtKinds(h.layerState.last!).has(LABEL_KIND.SHOT_LABEL)).toBe(true);
    // A FRESH controller at the same 13.2 builds section-only (below build 13.5).
    const fresh = cameraController(zoomOnlyMap(13.2).map);
    fresh.sync(makeParams());
    expect(builtKinds(h.layerState.last!)).toEqual(new Set([LABEL_KIND.SECTION_NAME]));
  });
});

describe('SurveyLabelController camera-settle rebuild (conditional)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('does NOT rebuild on a settle that stays inside the padded viewport (pan within margin)', () => {
    const cam = makeCameraMap(18);
    const c = cameraController(cam.map, { scheduleRebuild: (fn: () => void) => fn() });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = layer.updateCalls;
    cam.fire('moveend'); // camera unchanged → still covered → no rebuild
    expect(layer.updateCalls).toBe(before);
  });

  it('rebuilds on a settle that EXITS the padded viewport (a big pan)', () => {
    const cam = makeCameraMap(18);
    const c = cameraController(cam.map, { scheduleRebuild: (fn: () => void) => fn() });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = layer.updateCalls;
    cam.state.cLng += 0.05; // pan far east, out of the built padded viewport
    cam.fire('moveend');
    expect(layer.updateCalls).toBe(before + 1);
  });

  it('coalesces a moveend + zoomend burst into a single evaluation', () => {
    const cam = makeCameraMap(18);
    const hold = rebuildHolder();
    const c = cameraController(cam.map, {
      scheduleRebuild: (fn: () => void) => {
        hold.fn = fn;
        hold.count++;
      },
    });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = layer.updateCalls;
    cam.state.cLng += 0.05; // exit → the eventual evaluation will rebuild
    cam.fire('moveend');
    cam.fire('zoomend');
    // Only one evaluation was scheduled (the second event coalesced).
    expect(hold.count).toBe(1);
    hold.run();
    expect(layer.updateCalls).toBe(before + 1);
    // A later settle schedules a fresh evaluation.
    cam.fire('moveend');
    expect(hold.count).toBe(2);
  });

  it('never rebuilds mid-animation (isMoving) - waits for the next settle', () => {
    const cam = makeCameraMap(18);
    const hold = rebuildHolder();
    const c = cameraController(cam.map, {
      scheduleRebuild: (fn: () => void) => {
        hold.fn = fn;
      },
    });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = layer.updateCalls;
    cam.state.cLng += 0.05; // exit condition present
    cam.state.moving = true;
    cam.fire('moveend');
    hold.run(); // camera still animating → skip
    expect(layer.updateCalls).toBe(before);
    cam.state.moving = false;
    cam.fire('moveend');
    hold.run(); // settled + exited → rebuild
    expect(layer.updateCalls).toBe(before + 1);
  });

  it('an edit sync() rebuilds synchronously (no scheduler)', () => {
    const cam = makeCameraMap(18);
    let scheduled = 0;
    const c = cameraController(cam.map, { scheduleRebuild: () => scheduled++ });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = layer.updateCalls;
    c.sync(makeParams()); // an edit re-sync
    expect(layer.updateCalls).toBe(before + 1);
    expect(scheduled).toBe(0); // the edit path never defers
  });

  it('does not rebuild after destroy (listeners removed)', () => {
    const cam = makeCameraMap(18);
    const c = cameraController(cam.map, { scheduleRebuild: (fn: () => void) => fn() });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    c.destroy();
    const after = layer.updateCalls;
    cam.state.cLng += 0.05;
    expect(() => cam.fire('moveend')).not.toThrow();
    expect(layer.updateCalls).toBe(after);
  });

  it('rebuilds when a settle crosses the section-name minzoom (below 10 → into [10, band)) so section names appear', () => {
    // A build below SECTION_NAME_MIN_ZOOM (10) emits NO section names (empty
    // set). Zooming into [10, 13.5) stays inside the larger low-zoom padded
    // viewport, and wantFull is false on both sides of 10, so checks 1-3 all
    // miss it; without the minzoom-crossing check the settle would NOT rebuild
    // and section names would stay absent until 13.5 / an edit / a big pan.
    const cam = makeCameraMap(8);
    const c = cameraController(cam.map, { scheduleRebuild: (fn: () => void) => fn() });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    expect((layer.data as LabelRenderData).buffers.glyphCount).toBe(0); // nothing below 10
    expect(builtKinds(layer).has(LABEL_KIND.SECTION_NAME)).toBe(false);
    const before = layer.updateCalls;
    cam.state.zoom = 11; // into [10, band): still inside the padded z8 viewport
    cam.fire('zoomend');
    expect(layer.updateCalls).toBe(before + 1); // the crossing forced a rebuild
    expect(builtKinds(layer).has(LABEL_KIND.SECTION_NAME)).toBe(true); // names now present
  });

  it('does NOT rebuild on a settle that stays below the section-name minzoom', () => {
    // Both builds are below 10 (same band) → no crossing, and the smaller z9
    // viewport is still inside the padded z8 box → no rebuild.
    const cam = makeCameraMap(8);
    const c = cameraController(cam.map, { scheduleRebuild: (fn: () => void) => fn() });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = layer.updateCalls;
    cam.state.zoom = 9;
    cam.fire('zoomend');
    expect(layer.updateCalls).toBe(before);
  });
});

describe('SurveyLabelController NO rebuild on zoom-in (reveal-zoom admission)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('a modest zoom-in does NOT rebuild (the shader admits labels per frame)', () => {
    const cam = makeCameraMap(18);
    const c = cameraController(cam.map, { scheduleRebuild: (fn: () => void) => fn() });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = layer.updateCalls;
    // Zoom in half a level: the viewport shrinks (still inside the built padded
    // box). The shader admits more labels per frame from their reveal zooms - no
    // rebuild (true on either emit path for a modest step).
    cam.state.zoom = 18.5;
    cam.fire('zoomend');
    expect(layer.updateCalls).toBe(before);
  });

  it('a deep zoom-in after an ADAPTIVE (emit-all) build does NOT rebuild', () => {
    // Default (high) adaptiveEmitMax → the small fixture emits IN FULL to the
    // ceiling, so even a deep zoom-in has nothing more to admit: no rebuild. This
    // is the reported latency fix at high zoom.
    const cam = makeCameraMap(18);
    const c = cameraController(cam.map, { scheduleRebuild: (fn: () => void) => fn() });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = layer.updateCalls;
    cam.state.zoom = 19.5; // > 18 + 1, but the emitted set already reaches the ceiling
    cam.fire('zoomend');
    expect(layer.updateCalls).toBe(before);
  });

  it('a deep zoom-in past buildZoom + EMIT_SLACK/2 rebuilds once when the build was BOUNDED', () => {
    // Force the bounded-slack path (adaptiveEmitMax 0 = never emit-all) so the
    // emitted set has a finite horizon a deep zoom-in runs past → one rebuild.
    const cam = makeCameraMap(18);
    const c = cameraController(cam.map, { scheduleRebuild: (fn: () => void) => fn(), adaptiveEmitMax: 0 });
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = layer.updateCalls;
    cam.state.zoom = 19.5; // > 18 + 1
    cam.fire('zoomend');
    expect(layer.updateCalls).toBe(before + 1);
  });
});

describe('SurveyLabelController emit slack (vertex-load bound)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('a BOUNDED build emits fewer glyphs at a low zoom than a high one (labels past the horizon dropped)', () => {
    // Force the bounded-slack path (adaptiveEmitMax 0) so the vertex-load cap is
    // exercised on the small fixture. At z14 the fixture (10 m spacing) is densely
    // packed on screen → many labels reveal past buildZoom + slack (16) and are
    // NOT emitted. At z18 the same survey is spread out → higher horizon (20) →
    // strictly more glyphs. Correctness is never wrong: the missing low-zoom
    // labels appear after a settle/zoom-in rebuild.
    const low = cameraController(makeCameraMap(14).map, { adaptiveEmitMax: 0 });
    low.sync(makeParams());
    const lowGlyphs = (h.layerState.last!.data as LabelRenderData).buffers.glyphCount;

    const high = cameraController(makeCameraMap(18).map, { adaptiveEmitMax: 0 });
    high.sync(makeParams());
    const highGlyphs = (h.layerState.last!.data as LabelRenderData).buffers.glyphCount;

    expect(highGlyphs).toBeGreaterThan(lowGlyphs);
  });

  it('an ADAPTIVE build emits MORE than the bounded build at the same zoom (recovers dropped labels)', () => {
    // The reported fix: at a zoom where the bounded horizon drops dense-cluster /
    // band-edge labels, adaptive emit (default threshold) emits them all instead -
    // so a zoom-in reveals them with NO rebuild.
    const bounded = cameraController(makeCameraMap(14).map, { adaptiveEmitMax: 0 });
    bounded.sync(makeParams());
    const boundedGlyphs = (h.layerState.last!.data as LabelRenderData).buffers.glyphCount;

    const adaptive = cameraController(makeCameraMap(14).map); // default → emit-all
    adaptive.sync(makeParams());
    const adaptiveGlyphs = (h.layerState.last!.data as LabelRenderData).buffers.glyphCount;

    expect(adaptiveGlyphs).toBeGreaterThan(boundedGlyphs);
  });
});

describe('SurveyLabelController shape cache reuse', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('reuses shaped labels across camera rebuilds - same ShapedLabel object per (kind, text)', () => {
    // Spy on the atlas so we can prove a re-scope does NOT re-shape unchanged
    // labels: a cached (kind, text) hits the cache and never touches the atlas.
    const atlas = fakeGlyphAtlas();
    const getGlyph = vi.spyOn(atlas, 'getGlyph');
    const cam = makeCameraMap(18);
    const c = new SurveyLabelController(cam.map, undefined, {
      atlas,
      scheduleRebuild: (fn: () => void) => fn(),
    });
    c.sync(makeParams());
    const afterFirst = getGlyph.mock.calls.length;
    expect(afterFirst).toBeGreaterThan(0); // shaped on the first build
    // A camera-settle rebuild that EXITS the viewport (so build() runs again) on
    // the SAME geometry must not re-shape - every (kind, text) is cache-served.
    cam.state.cLng += 0.05;
    cam.fire('moveend');
    expect(getGlyph.mock.calls.length).toBe(afterFirst); // zero further shaping
  });
});

// ─── dim + drag-hide flags, reveal rerun, synchronous placement ───────────────

function vfl(layer: FakeLabelLayer): { kind: Uint8Array; revealZoom: Float32Array; flags: Uint8Array } {
  const d = layer.data as LabelRenderData;
  return { kind: d.buffers.kind, revealZoom: d.buffers.revealZoom, flags: d.buffers.flags };
}

/** Does ANY vertex of `kind` have `bit` set? */
function anyKindHasBit(layer: FakeLabelLayer, kind: number, bit: number): boolean {
  const { kind: k, flags } = vfl(layer);
  for (let v = 0; v < k.length; v++) if (k[v] === kind && (flags[v]! & bit) !== 0) return true;
  return false;
}

/** Does EVERY vertex of `kind` have `bit` set (false if the kind is absent)? */
function allKindHaveBit(layer: FakeLabelLayer, kind: number, bit: number): boolean {
  const { kind: k, flags } = vfl(layer);
  let seen = false;
  for (let v = 0; v < k.length; v++) {
    if (k[v] !== kind) continue;
    seen = true;
    if ((flags[v]! & bit) === 0) return false;
  }
  return seen;
}

/** Anchor-relative mercator of station global `g` (per-vertex ordinal is gone
 *  from the buffer, so labels are identified by their anchor - deterministic
 *  from the fixture coords). */
function stationAnchor(params: LabelSyncParams, g: number): [number, number] {
  const [ax, ay] = computeMercatorAnchor(params.surveyData, params.gpsOrigin);
  const rel = projectStationsRel(params.surveyData, params.gpsOrigin, ax, ay);
  return [rel[g * 2]!, rel[g * 2 + 1]!];
}

/** Flag byte of the station label whose anchor matches station `g` (0 if absent). */
function stationLabelFlag(layer: FakeLabelLayer, params: LabelSyncParams, g: number): number {
  const [ax, ay] = stationAnchor(params, g);
  const d = layer.data as LabelRenderData;
  const { kind, flags, anchor } = d.buffers;
  for (let v = 0; v < kind.length; v++) {
    if (kind[v] !== LABEL_KIND.STATION_LABEL) continue;
    if (Math.abs(anchor[v * 2]! - ax) < 1e-9 && Math.abs(anchor[v * 2 + 1]! - ay) < 1e-9) return flags[v]!;
  }
  return 0;
}

/** Flag byte of the SECTION NAME anchored at the section's first station
 *  `firstStationG` (names anchor at the first non-reference station); 0 if
 *  absent. Lets a test target one section's name among several. */
function sectionNameFlag(layer: FakeLabelLayer, params: LabelSyncParams, firstStationG: number): number {
  const [ax, ay] = stationAnchor(params, firstStationG);
  const d = layer.data as LabelRenderData;
  const { kind, flags, anchor } = d.buffers;
  for (let v = 0; v < kind.length; v++) {
    if (kind[v] !== LABEL_KIND.SECTION_NAME) continue;
    if (Math.abs(anchor[v * 2]! - ax) < 1e-9 && Math.abs(anchor[v * 2 + 1]! - ay) < 1e-9) return flags[v]!;
  }
  return 0;
}

const EMPTY_DIM = {
  anchoredSectionIds: new Set<number>(),
  immovableStationFeatureIds: new Set<number>(),
  immovableShotFeatureIds: new Set<number>(),
  gasDimStationFeatureIds: new Set<number>(),
  gasDimShotFeatureIds: new Set<number>(),
  planIdleDim: false,
};

/** Fixture + a station comment on g0 (section 1) and a shot comment on shot 0. */
function commentParams(): LabelSyncParams {
  const p = makeParams();
  p.surveyData.stationComments = new Map([[0, 'stn']]);
  p.surveyData.shotComments = new Map([[0, 'sht']]);
  return p;
}

describe('SurveyLabelController dim parity (DIM flag)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('anchored dim: shot/station/section labels dim, comments do NOT (stock parity)', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(commentParams());
    const layer = h.layerState.last!;
    // Anchor BOTH sections so every label of a dimmable kind is covered.
    c.setDimming({ ...EMPTY_DIM, anchoredSectionIds: new Set([1, 2]) });
    expect(layer.flagsCalls).toBeGreaterThan(0); // cheap flags-only refresh, no rebuild
    expect(allKindHaveBit(layer, LABEL_KIND.SHOT_LABEL, LABEL_FLAG.DIM)).toBe(true);
    expect(allKindHaveBit(layer, LABEL_KIND.STATION_LABEL, LABEL_FLAG.DIM)).toBe(true);
    expect(allKindHaveBit(layer, LABEL_KIND.SECTION_NAME, LABEL_FLAG.DIM)).toBe(true);
    // Comments are never dimmed by stock label expressions.
    expect(anyKindHasBit(layer, LABEL_KIND.STATION_COMMENT, LABEL_FLAG.DIM)).toBe(false);
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_COMMENT, LABEL_FLAG.DIM)).toBe(false);
  });

  it('immovable dim: only the immovable station label dims; section names never do', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    const p = commentParams();
    c.sync(p);
    const layer = h.layerState.last!;
    // Immovable station g0 == section 1, station index 0.
    c.setDimming({
      ...EMPTY_DIM,
      immovableStationFeatureIds: new Set([1 * FEATURE_ID_STRIDE + 0]),
    });
    expect(stationLabelFlag(layer, p, 0) & LABEL_FLAG.DIM).toBe(LABEL_FLAG.DIM); // g0 dims
    expect(stationLabelFlag(layer, p, 1) & LABEL_FLAG.DIM).toBe(0); // g1 does not
    // Section names are not immovable-dimmed.
    expect(anyKindHasBit(layer, LABEL_KIND.SECTION_NAME, LABEL_FLAG.DIM)).toBe(false);
  });

  it('plan-idle tint (#395 revision): EVERY label dims, including comments', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(commentParams());
    const layer = h.layerState.last!;
    c.setDimming({ ...EMPTY_DIM, planIdleDim: true });
    expect(allKindHaveBit(layer, LABEL_KIND.SHOT_LABEL, LABEL_FLAG.DIM)).toBe(true);
    expect(allKindHaveBit(layer, LABEL_KIND.STATION_LABEL, LABEL_FLAG.DIM)).toBe(true);
    expect(allKindHaveBit(layer, LABEL_KIND.SECTION_NAME, LABEL_FLAG.DIM)).toBe(true);
    // Unlike anchored dim, the idle tint reaches comments too - it mirrors the
    // "whole survey tints" geometry behavior, so nothing stays full-bright.
    expect(allKindHaveBit(layer, LABEL_KIND.STATION_COMMENT, LABEL_FLAG.DIM)).toBe(true);
    expect(allKindHaveBit(layer, LABEL_KIND.SHOT_COMMENT, LABEL_FLAG.DIM)).toBe(true);
    // Clearing the flag drops back to full-bright.
    c.setDimming(EMPTY_DIM);
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_LABEL, LABEL_FLAG.DIM)).toBe(false);
    expect(anyKindHasBit(layer, LABEL_KIND.STATION_COMMENT, LABEL_FLAG.DIM)).toBe(false);
  });

  it('clearing the dim set removes DIM (flags-only)', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(commentParams());
    const layer = h.layerState.last!;
    c.setDimming({ ...EMPTY_DIM, anchoredSectionIds: new Set([1, 2]) });
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_LABEL, LABEL_FLAG.DIM)).toBe(true);
    c.setDimming(EMPTY_DIM);
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_LABEL, LABEL_FLAG.DIM)).toBe(false);
  });
});

describe('SurveyLabelController drag-hide parity (DRAG_HIDE flag)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  // PARITY BREAK (deliberate): stock left `survey-labels` out of the drag hide
  // layers, so a section name stayed pinned at its old anchor while the geometry
  // moved (reads as stuck). The custom label layer now DRAG_HIDEs the name of
  // every AFFECTED section - the whole-section drag via `sectionIds`, a partial
  // station / shot drag via the `affectedSectionIds` set SurveyLayer threads in.
  it('hides the dragged station label + its comment + its section name; leaves other stations and unaffected section names', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    const p = commentParams();
    c.sync(p);
    const layer = h.layerState.last!;
    // Drag station g0 (section 1); SurveyLayer marks section 1 as affected.
    c.setDragHide({ stationGlobalIndices: new Set([0]), affectedSectionIds: new Set([1]) });
    expect(layer.flagsCalls).toBeGreaterThan(0);
    expect(stationLabelFlag(layer, p, 0) & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
    expect(stationLabelFlag(layer, p, 1) & LABEL_FLAG.DRAG_HIDE).toBe(0);
    // The station comment (only g0 has one) hides with its station.
    expect(anyKindHasBit(layer, LABEL_KIND.STATION_COMMENT, LABEL_FLAG.DRAG_HIDE)).toBe(true);
    // Section 1's name (first station g0) hides; section 2's name (g3) does not.
    expect(sectionNameFlag(layer, p, 0) & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
    expect(sectionNameFlag(layer, p, 3) & LABEL_FLAG.DRAG_HIDE).toBe(0);
  });

  it('does NOT hide a section name whose section is not in affectedSectionIds', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    const p = commentParams();
    c.sync(p);
    const layer = h.layerState.last!;
    // Drag station g0 but omit the affected-section set (station-only exclusion):
    // the station label hides, its section name does NOT.
    c.setDragHide({ stationGlobalIndices: new Set([0]) });
    expect(stationLabelFlag(layer, p, 0) & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
    expect(sectionNameFlag(layer, p, 0) & LABEL_FLAG.DRAG_HIDE).toBe(0);
  });

  it('hides the dragged shot label + its comment', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(commentParams());
    const layer = h.layerState.last!;
    c.setDragHide({ shotGlobalIndices: new Set([0]) }); // drag shot global 0
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_LABEL, LABEL_FLAG.DRAG_HIDE)).toBe(true);
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_COMMENT, LABEL_FLAG.DRAG_HIDE)).toBe(true);
  });

  it('a whole-section drag hides that section labels AND its section name', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    const p = commentParams();
    c.sync(p);
    const layer = h.layerState.last!;
    c.setDragHide({ sectionIds: new Set([1]) });
    expect(anyKindHasBit(layer, LABEL_KIND.STATION_LABEL, LABEL_FLAG.DRAG_HIDE)).toBe(true);
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_LABEL, LABEL_FLAG.DRAG_HIDE)).toBe(true);
    // Section 1's name hides with the whole section; section 2's stays visible.
    expect(sectionNameFlag(layer, p, 0) & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
    expect(sectionNameFlag(layer, p, 3) & LABEL_FLAG.DRAG_HIDE).toBe(0);
  });

  it('setDragHide(null) clears every DRAG_HIDE incl section names (drag end)', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    const p = commentParams();
    c.sync(p);
    const layer = h.layerState.last!;
    c.setDragHide({ stationGlobalIndices: new Set([0]), affectedSectionIds: new Set([1]) });
    expect(stationLabelFlag(layer, p, 0) & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
    expect(sectionNameFlag(layer, p, 0) & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
    c.setDragHide(null);
    expect(anyKindHasBit(layer, LABEL_KIND.STATION_LABEL, LABEL_FLAG.DRAG_HIDE)).toBe(false);
    expect(anyKindHasBit(layer, LABEL_KIND.SECTION_NAME, LABEL_FLAG.DRAG_HIDE)).toBe(false);
  });

  it('DIM and DRAG_HIDE compose without disturbing each other', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    const p = commentParams();
    c.sync(p);
    const layer = h.layerState.last!;
    c.setDimming({ ...EMPTY_DIM, anchoredSectionIds: new Set([1]) });
    c.setDragHide({ stationGlobalIndices: new Set([0]) });
    // g0 has both bits; the dim persists through the drag flag upload.
    const f0 = stationLabelFlag(layer, p, 0);
    expect(f0 & LABEL_FLAG.DIM).toBe(LABEL_FLAG.DIM);
    expect(f0 & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
    // g1 (section 1, not dragged) keeps DIM but not DRAG_HIDE.
    const f1 = stationLabelFlag(layer, p, 1);
    expect(f1 & LABEL_FLAG.DIM).toBe(LABEL_FLAG.DIM);
    expect(f1 & LABEL_FLAG.DRAG_HIDE).toBe(0);
  });

  it('re-applies drag-hide on a mid-drag rebuild (state survives build)', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    const p = commentParams();
    c.sync(p);
    const layer = h.layerState.last!;
    c.setDragHide({ stationGlobalIndices: new Set([0]), affectedSectionIds: new Set([1]) });
    // A mid-drag geometry flush re-syncs; the fresh build must re-bake DRAG_HIDE
    // for both the dragged station label AND its affected section name.
    c.sync(commentParams());
    expect(stationLabelFlag(layer, p, 0) & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
    expect(sectionNameFlag(layer, p, 0) & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
  });
});

// The fixture puts a station comment on station g0 (section 1, station index 0)
// and a shot comment on shot global 0 (section 1, shot id 0), so the hidden-comment
// identity is {sectionId:1, id:0} for both kinds.
describe('SurveyLabelController hidden-comment parity (COMMENT_HIDE flag, C3)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('hides ONLY the picked-up shot comment; leaves the station comment + all labels', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(commentParams());
    const layer = h.layerState.last!;
    c.setHiddenComment({ type: 'shot', sectionId: 1, id: 0 });
    expect(layer.flagsCalls).toBeGreaterThan(0); // cheap flags-only refresh, no rebuild
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_COMMENT, LABEL_FLAG.COMMENT_HIDE)).toBe(true);
    // A station/shot LABEL sharing the anchor, and the OTHER comment kind, never hide.
    expect(anyKindHasBit(layer, LABEL_KIND.STATION_COMMENT, LABEL_FLAG.COMMENT_HIDE)).toBe(false);
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_LABEL, LABEL_FLAG.COMMENT_HIDE)).toBe(false);
    expect(anyKindHasBit(layer, LABEL_KIND.STATION_LABEL, LABEL_FLAG.COMMENT_HIDE)).toBe(false);
  });

  it('hides ONLY the picked-up station comment', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(commentParams());
    const layer = h.layerState.last!;
    c.setHiddenComment({ type: 'station', sectionId: 1, id: 0 });
    expect(anyKindHasBit(layer, LABEL_KIND.STATION_COMMENT, LABEL_FLAG.COMMENT_HIDE)).toBe(true);
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_COMMENT, LABEL_FLAG.COMMENT_HIDE)).toBe(false);
  });

  it('does not match a comment in a different section (id/section keyed)', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(commentParams());
    const layer = h.layerState.last!;
    c.setHiddenComment({ type: 'shot', sectionId: 2, id: 0 }); // section 2 has no comment here
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_COMMENT, LABEL_FLAG.COMMENT_HIDE)).toBe(false);
  });

  it('setHiddenComment(null) clears COMMENT_HIDE (drop / cancel)', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(commentParams());
    const layer = h.layerState.last!;
    c.setHiddenComment({ type: 'shot', sectionId: 1, id: 0 });
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_COMMENT, LABEL_FLAG.COMMENT_HIDE)).toBe(true);
    c.setHiddenComment(null);
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_COMMENT, LABEL_FLAG.COMMENT_HIDE)).toBe(false);
  });

  it('re-applies COMMENT_HIDE on a mid-pickup rebuild (state survives build)', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(commentParams());
    const layer = h.layerState.last!;
    c.setHiddenComment({ type: 'shot', sectionId: 1, id: 0 });
    c.sync(commentParams()); // a geometry flush during pickup
    expect(anyKindHasBit(layer, LABEL_KIND.SHOT_COMMENT, LABEL_FLAG.COMMENT_HIDE)).toBe(true);
  });
});

describe('SurveyLabelController synchronous placement (undo/redo)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('a sync with moved coordinates places labels at the new anchor in the SAME call', () => {
    const cam = zoomOnlyMap(18);
    const c = cameraController(cam.map);
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const before = (layer.data as LabelRenderData).anchorX;

    // Move every station 5 m in x (an undo/redo of a move re-flushes new SoA).
    const moved = makeParams();
    for (let g = 0; g < moved.surveyData.stationCount; g++) {
      moved.surveyData.stationCoords[g * 3] = moved.surveyData.stationCoords[g * 3]! + 5;
    }
    c.sync(moved);
    // The build ran synchronously in this call (new data on the layer) and the
    // survey anchor shifted with the moved coordinates - no deferred convergence.
    expect(layer.updateCalls).toBeGreaterThan(0);
    expect((layer.data as LabelRenderData).anchorX).not.toBeCloseTo(before, 12);
  });
});

describe('SurveyLabelController reveal rerun on toggle', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('a kind toggle reruns the reveal pass (reveal-only upload), NOT a buffer rebuild', () => {
    const cam = makeCameraMap(18); // has bounds → reveal + rerunReveal actually run
    const c = cameraController(cam.map);
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const revealBefore = layer.revealCalls;
    const updatesBefore = layer.updateCalls;
    c.setShotLabelsVisible(false);
    // Declutter re-ran via a reveal-only upload - NOT a buffer rebuild.
    expect(layer.revealCalls).toBeGreaterThan(revealBefore);
    expect(layer.updateCalls).toBe(updatesBefore);
  });

  it('a reference-shots toggle reruns the reveal pass (reveal-only upload)', () => {
    const cam = makeCameraMap(18);
    const c = cameraController(cam.map);
    c.sync(makeParams());
    const layer = h.layerState.last!;
    const revealBefore = layer.revealCalls;
    const updatesBefore = layer.updateCalls;
    c.setReferenceShotsVisible(false);
    expect(layer.revealCalls).toBeGreaterThan(revealBefore);
    expect(layer.updateCalls).toBe(updatesBefore);
  });
});

describe('SurveyLabelController rerun keeps a lost virtual label hidden', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  /** Section B's one shot is a Jump laid exactly over section A's first (real)
   *  shot, so their shot labels share an anchor and overlap at every zoom. */
  function jumpOverRealParams(): LabelSyncParams {
    const params = makeParams();
    const d = params.surveyData;
    d.stationCount = 5;
    d.stationSectionIds = new Uint32Array([1, 1, 1, 2, 2]);
    d.stationIndices = new Uint32Array([0, 1, 2, 0, 1]);
    d.stationCoords = new Float64Array([0, 0, 0, 10, 0, -1, 10, 10, -2, 0, 0, 0, 10, 0, -1]);
    d.stationFlags = new Uint8Array([STATION_FLAG_IS_ZERO_DATUM, 0, 0, 0, 0]);
    d.shotFlags = new Uint8Array([0, 0, shotKindToFlags('Jump')]);
    return params;
  }

  /** Reveal zooms of the shot-label vertices, split by whether they carry JUMP. */
  function shotLabelReveals(layer: FakeLabelLayer): { real: number[]; jump: number[] } {
    const { kind, revealZoom, flags } = vfl(layer);
    const out = { real: [] as number[], jump: [] as number[] };
    for (let v = 0; v < kind.length; v++) {
      if (kind[v] !== LABEL_KIND.SHOT_LABEL) continue;
      ((flags[v]! & LABEL_FLAG.JUMP) !== 0 ? out.jump : out.real).push(revealZoom[v]!);
    }
    return out;
  }

  it('re-showing jumps hides the jump label that loses to a real one, rather than drawing it on top at the horizon', () => {
    const cam = makeCameraMap(18);
    const c = cameraController(cam.map);
    c.setShotLabelsVisible(true);
    // Built with jumps hidden: the jump label is no candidate, so it is emitted
    // uncontested (the shader hides it while jumps are off).
    c.setJumpShotsVisible(false);
    c.sync(jumpOverRealParams());
    const layer = h.layerState.last!;
    const built = shotLabelReveals(layer);
    expect(built.jump.length).toBeGreaterThan(0);
    const realBefore = built.real.slice();

    c.setJumpShotsVisible(true);
    const after = shotLabelReveals(layer);
    // The jump label now loses its slot to the coincident real label: hidden at
    // every zoom the map reaches, not parked at the emit horizon (z22 here).
    for (const r of after.jump) expect(r).toBeGreaterThan(REVEAL_CEILING);
    // The real labels keep their reveals.
    expect(after.real).toEqual(realBefore);
    expect(Math.max(...after.real)).toBeLessThanOrEqual(REVEAL_CEILING);
  });
});

describe('SurveyLabelController jump section names below the label band', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  /** Real section A (stations at x = -3000, 0, 10) and a jump section J tied in at
   *  A's middle station: J's name sits on A's station label there. */
  function tiedJumpParams(): LabelSyncParams {
    const params = makeParams();
    const d = params.surveyData;
    d.sections = [
      { id: 1, name: 'A', surveyor: null, date: null, color: '#ff0000', direction: 'in' as never, quality: 0, next_shot_id: 2, shotOffset: 0, shotCount: 2 },
      { id: 2, name: 'Jump to the downstream sump', surveyor: null, date: null, color: '#00ff00', direction: 'in' as never, quality: 0, next_shot_id: 1, shotOffset: 2, shotCount: 1 },
    ];
    d.shotCount = 3;
    d.shotIds = new Uint32Array([0, 1, 0]);
    d.shotSectionIds = new Uint32Array([1, 1, 2]);
    d.shotFlags = new Uint8Array([0, 0, shotKindToFlags('Jump')]);
    d.stationCount = 5;
    d.stationSectionIds = new Uint32Array([1, 1, 1, 2, 2]);
    d.stationIndices = new Uint32Array([0, 1, 2, 0, 1]);
    d.stationCoords = new Float64Array([-3000, 0, 0, 0, 0, -1, 10, 0, -2, 0, 0, -1, 0, 30, -1]);
    d.stationFlags = new Uint8Array([STATION_FLAG_IS_ZERO_DATUM, 0, 0, 0, 0]);
    return params;
  }

  /** Reveal zooms of J's section name (the all-jump one), [] when not emitted. */
  function jumpNameReveals(layer: FakeLabelLayer): number[] {
    const { kind, revealZoom, flags } = vfl(layer);
    const out: number[] = [];
    for (let v = 0; v < kind.length; v++) {
      if (kind[v] === LABEL_KIND.SECTION_NAME && (flags[v]! & LABEL_FLAG.ALL_JUMP) !== 0) out.push(revealZoom[v]!);
    }
    return out;
  }

  it('shows at z12 while station labels are off, and is left out once they are on - as the full build leaves it', () => {
    const cam = makeCameraMap(12);
    const c = cameraController(cam.map);
    c.sync(tiedJumpParams());
    const layer = h.layerState.last!;
    // Nothing to leave free: J's name shows below the band.
    expect(jumpNameReveals(layer).length).toBeGreaterThan(0);
    expect(Math.max(...jumpNameReveals(layer))).toBeLessThan(13.5);

    // Station labels on: A's station label (drawn from the band up) outranks
    // J's name, so the name is not shown below the band either. It stays in the
    // buffer, hidden at every zoom the map reaches, so a reveal-only rerun
    // still sees the slot it holds.
    const hidden = (r: number[]) => r.length > 0 && r.every((z) => z > REVEAL_CEILING);
    c.setStationLabelMode('id');
    expect(hidden(jumpNameReveals(layer))).toBe(true);
    // A rerun (jumps hidden, then shown again) leaves it hidden.
    c.setJumpShotsVisible(false);
    c.setJumpShotsVisible(true);
    expect(hidden(jumpNameReveals(layer))).toBe(true);

    // Past the band, the full build agrees: the name never shows there.
    cam.state.zoom = 14;
    c.sync(tiedJumpParams());
    // (A full build reserves nothing, so the name is simply not emitted.)
    expect(jumpNameReveals(h.layerState.last!)).toEqual([]);
  });

  // Station names mode (#697) labels only named stations, with longer boxes:
  // the reservation must build the same text as the full build.
  it('in names mode it reserves the names a full build draws, not IDs', () => {
    const cam = makeCameraMap(12);
    const c = cameraController(cam.map);
    c.setStationLabelMode('name');
    // No station is named, so the full build draws no station label at the
    // tie-in and nothing holds J's name back, below the band or past it.
    c.sync(tiedJumpParams());
    expect(Math.max(...jumpNameReveals(h.layerState.last!))).toBeLessThan(13.5);
    cam.state.zoom = 14;
    c.sync(tiedJumpParams());
    expect(jumpNameReveals(h.layerState.last!).length).toBeGreaterThan(0);

    // Name the tie-in station (A's g1): its name outranks J's, below the band too.
    const named = tiedJumpParams();
    named.surveyData.stationNames = new Map([[1, 'Tie-in at the restriction']]);
    cam.state.zoom = 12;
    c.sync(named);
    const reveals = jumpNameReveals(h.layerState.last!);
    expect(reveals.length > 0 && reveals.every((z) => z > REVEAL_CEILING)).toBe(true);
  });
});

describe('SurveyLabelController labelRestore-guard contract (isActive gates the guard)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  // SurveyLayer guards every stock labelRestore call with `!customLabels.isActive()`.
  // Lock the controller side of that predicate across enable + teardown.
  it('isActive() is true only after a build has landed, false again after destroy', () => {
    const cam = makeCameraMap(18);
    const c = cameraController(cam.map);
    expect(c.isActive()).toBe(false); // nothing synced yet → guard runs stock restore
    c.sync(makeParams());
    expect(c.isActive()).toBe(true); // active → guard SKIPS stock restore
    c.destroy();
    expect(c.isActive()).toBe(false); // torn down → guard runs stock restore again
  });
});

describe('SurveyLabelController station label mode (Station IDs / Station names)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  /** The fixture with names on g1 ('AB') and g3 ('C'); g0 and g2 are unnamed. */
  function namedParams(): LabelSyncParams {
    const p = makeParams();
    p.surveyData.stationNames = new Map([[1, 'AB'], [3, 'C']]);
    return p;
  }

  /** Station-label glyphs in the last-built buffer (one quad = 4 vertices each). */
  function stationGlyphs(layer: FakeLabelLayer): number {
    return vfl(layer).kind.filter((k) => k === LABEL_KIND.STATION_LABEL).length / 4;
  }

  it('switching IDs -> names rebuilds once and labels only the named stations', () => {
    const c = cameraController(zoomOnlyMap(18).map);
    c.setStationLabelMode('id');
    const p = namedParams();
    c.sync(p);
    const layer = h.layerState.last!;
    expect(stationGlyphs(layer)).toBe(4); // "0", "1", "2", "0"
    const updates = layer.updateCalls;

    c.setStationLabelMode('name');
    // The text is baked into the glyph quads, so a text switch is a rebuild.
    expect(layer.updateCalls).toBe(updates + 1);
    expect(stationGlyphs(layer)).toBe(3); // "AB", "C"; g0 and g2 get no label
    expect(stationLabelFlag(layer, p, 0)).toBe(0);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.STATION_LABEL]).toBe(true);

    c.setStationLabelMode('id');
    expect(layer.updateCalls).toBe(updates + 2);
    expect(stationGlyphs(layer)).toBe(4);
  });

  it('turning the current labels off and on is a uniform flip, not a rebuild', () => {
    const c = cameraController(zoomOnlyMap(18).map);
    c.sync(namedParams());
    const layer = h.layerState.last!;
    const updates = layer.updateCalls;

    c.setStationLabelMode('id'); // the default text: just show it
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.STATION_LABEL]).toBe(true);
    c.setStationLabelMode('off');
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.STATION_LABEL]).toBe(false);
    expect(layer.updateCalls).toBe(updates);

    c.setStationLabelMode('name'); // off -> names changes the text: one rebuild
    expect(layer.updateCalls).toBe(updates + 1);
    c.setStationLabelMode('off'); // hiding keeps the names built...
    c.setStationLabelMode('name'); // ...so showing them again needs no rebuild
    expect(layer.updateCalls).toBe(updates + 1);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.STATION_LABEL]).toBe(true);
    expect(stationGlyphs(layer)).toBe(3);
  });

  it('a mode set before the first build applies to that build', () => {
    const c = cameraController(zoomOnlyMap(18).map);
    c.setStationLabelMode('name');
    c.sync(namedParams());
    const layer = h.layerState.last!;
    expect(stationGlyphs(layer)).toBe(3);
    expect(layer.lastStyle?.kindEnabled[LABEL_KIND.STATION_LABEL]).toBe(true);
  });

  it('names mode follows the data: a later sync builds the new names', () => {
    const c = cameraController(zoomOnlyMap(18).map);
    c.setStationLabelMode('name');
    c.sync(namedParams());
    const layer = h.layerState.last!;
    const renamed = namedParams();
    renamed.surveyData.stationNames = new Map([[0, 'WXYZ']]);
    c.sync(renamed);
    expect(stationGlyphs(layer)).toBe(4);
    expect(stationLabelFlag(layer, renamed, 0)).not.toBe(0); // g0 is labelled now...
    expect(stationLabelFlag(layer, renamed, 1)).toBe(0); // ...and g1 no longer is
  });

  it('name labels carry the dim and drag-hide bits like the ID labels', () => {
    const c = cameraController(zoomOnlyMap(18).map);
    const p = namedParams();
    c.sync(p);
    const layer = h.layerState.last!;
    // Dim station g1 (section 1, index 1) while IDs show, then switch to names:
    // the rebuild recomputes the full flag byte, so the name label is dimmed too.
    c.setDimming({ ...EMPTY_DIM, immovableStationFeatureIds: new Set([1 * FEATURE_ID_STRIDE + 1]) });
    c.setStationLabelMode('name');
    expect(stationLabelFlag(layer, p, 1) & LABEL_FLAG.DIM).toBe(LABEL_FLAG.DIM);
    expect(stationLabelFlag(layer, p, 3) & LABEL_FLAG.DIM).toBe(0);
    c.setDragHide({ stationGlobalIndices: new Set([3]) });
    expect(stationLabelFlag(layer, p, 3) & LABEL_FLAG.DRAG_HIDE).toBe(LABEL_FLAG.DRAG_HIDE);
    expect(stationLabelFlag(layer, p, 1) & LABEL_FLAG.DRAG_HIDE).toBe(0);
  });
});
