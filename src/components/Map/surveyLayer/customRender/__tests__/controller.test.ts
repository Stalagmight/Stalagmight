import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import {
  emptySurveyData,
  STATION_FLAG_IS_ZERO_DATUM,
  type SurveyData,
} from '../../../../../types/survey-data';
import {
  SurveyCustomController,
  buildCustomDimState,
  GAS_DIM_LEVEL,
  type CustomSyncParams,
} from '../controller';
import type { CustomRenderData } from '../surveyCustomLayer';
import { CUSTOM_RENDER_SYNC_HOOK } from '../gate';

/**
 * Controller error-state tests (#customRender). The custom layer is the SOLE
 * survey renderer (no gate, no stock fallback). The GL layer is replaced with a
 * scriptable fake so we can drive the two UNRECOVERABLE failure modes:
 *  - onAdd disabled itself  -> onFatal fires, dead layer removed, no stock restore
 *  - GL error mid-render     -> deferred onFatal, dead layer removed, no restore
 * The pure buffer build (tessellate/mercator/colorPack/features) runs for real.
 */

// Shared, hoisted state so the mocked module + the test body see the same object.
const h = vi.hoisted(() => {
  const layerState = {
    disabledAfterAdd: false,
    disabledReason: 'program-link-failed' as string,
    last: null as FakeLayer | null,
  };

  class FakeLayer {
    readonly id: string;
    data: unknown;
    onSelfDisable: ((reason: string) => void) | null;
    private disabled = false;
    private reason: string | null = null;

    constructor(id: string, data: unknown, onSelfDisable: ((reason: string) => void) | null = null) {
      this.id = id;
      this.data = data;
      this.onSelfDisable = onSelfDisable;
      layerState.last = this;
    }

    /** Modelled after MapLibre calling onAdd inside addLayer(). */
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

    updateCalls = 0;
    update(data: unknown): void {
      this.data = data;
      this.updateCalls++;
    }

    // Display toggles the controller re-applies on enable + on setShow* calls.
    shotsVisible = true;
    stationsVisible = true;
    referenceShotsVisible = true;
    setShotsVisible(v: boolean): void {
      this.shotsVisible = v;
    }
    setStationsVisible(v: boolean): void {
      this.stationsVisible = v;
    }
    setReferenceShotsVisible(v: boolean): void {
      this.referenceShotsVisible = v;
    }
    jumpShotsVisible = true;
    setJumpShotsVisible(v: boolean): void {
      this.jumpShotsVisible = v;
    }

    // Buffer sub-uploads the controller's incremental / color-only fast paths
    // call; the fake just records that they ran (no GL).
    updateColorsCalls = 0;
    updateColors(): void {
      this.updateColorsCalls++;
    }
    updateDim(): void {}
    updateHide(): void {}
    updateHighlight(): void {}
    uploadCalls = 0;
    uploadLineVertexRange(): void {
      this.uploadCalls++;
    }
    uploadCircleVertexRange(): void {
      this.uploadCalls++;
    }
  }

  return { layerState, FakeLayer };
});

vi.mock('../surveyCustomLayer', () => ({ SurveyCustomLayer: h.FakeLayer }));

const STOCK = ['survey-shots', 'survey-shot-gradients', 'survey-stations'] as const;
const CUSTOM_ID = 'survey-custom-render';

interface FakeMap {
  map: MaplibreMap;
  visibility: Record<string, string>;
  removed: string[];
}

function makeFakeMap(): FakeMap {
  const existing = new Set<string>([...STOCK]);
  const visibility: Record<string, string> = {
    'survey-shots': 'visible',
    'survey-shot-gradients': 'visible',
    'survey-stations': 'visible',
  };
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

/** Two-section fixture identical in shape to the tessellation suite's. */
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

function makeParams(): CustomSyncParams {
  const themeColors = {
    stationDefault: '#ffffff',
    zeroDatum: '#00ffff',
    strokeColor: '#000000',
    depthShallow: '#0000ff',
    depthDeep: '#ff0000',
    severityError: '#ff0000',
    severityWarning: '#ffff00',
    severityClear: '#00000000',
  };
  const colorCtx = {
    showDepthGradient: false,
    showIssueSeverity: false,
    depthRange: { min: 0, max: 1 },
    themeColors,
  } as unknown as CustomSyncParams['colorCtx'];
  return {
    surveyData: makeFixture(),
    gpsOrigin: null,
    colorCtx,
    shotWidthPx: 2,
    stationRadiusPx: 3,
    stationStrokePx: 1,
  };
}

describe('SurveyCustomController error state (no stock fallback)', () => {
  let warn: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.disabledReason = 'program-link-failed';
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => vi.restoreAllMocks());

  it('enables the custom layer on a clean init (sole renderer, no stock to hide)', () => {
    const { map, removed } = makeFakeMap();
    const c = new SurveyCustomController(map);
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);
    expect(removed).not.toContain(CUSTOM_ID);
    expect(warn).not.toHaveBeenCalled();
  });

  it('onAdd-disabled: fires onFatal, removes the dead layer, does NOT restore stock', () => {
    h.layerState.disabledAfterAdd = true;
    h.layerState.disabledReason = 'webgl2-unavailable';
    const fatals: string[] = [];
    const { map, visibility, removed } = makeFakeMap();
    const c = new SurveyCustomController(map, undefined, { onFatal: (r) => fatals.push(r) });
    c.sync(makeParams());
    // Error state: dead layer removed, onFatal fired with the reason. There is no
    // stock fallback, so the controller never TOUCHES the stock layers (they stay
    // at their untouched initial visibility - not deliberately restored).
    expect(removed).toContain(CUSTOM_ID);
    expect(fatals).toEqual(['webgl2-unavailable']);
    expect(warn).toHaveBeenCalledOnce();
    expect(warn.mock.calls[0]?.[0]).toContain('webgl2-unavailable');
    expect(warn.mock.calls[0]?.[0]).toContain('survey rendering unavailable');
    // A later sync() is a no-op: the fatal latch keeps the survey blank.
    void visibility;
    c.sync(makeParams());
    expect(removed.filter((id) => id === CUSTOM_ID).length).toBe(1);
    expect(fatals).toEqual(['webgl2-unavailable']); // onFatal fires at most once
  });

  it('render-error self-disable fires onFatal (deferred), removes the layer', () => {
    // Object holder (not a bare `let`) so TS control-flow doesn't narrow away
    // the deferred assignment; also lets us count how often it was scheduled.
    const holder: { fn: (() => void) | null; scheduled: number } = { fn: null, scheduled: 0 };
    const fatals: string[] = [];
    const { map, removed } = makeFakeMap();
    const c = new SurveyCustomController(map, undefined, {
      onFatal: (r) => fatals.push(r),
      scheduleRestore: (fn) => {
        holder.fn = fn;
        holder.scheduled += 1;
      },
    });
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);

    const layer = h.layerState.last;
    expect(layer).not.toBeNull();
    // Simulate the layer self-disabling mid-render.
    layer?.onSelfDisable?.('gl-error-0x502');
    // Deferred: nothing torn down yet (must not mutate style mid-render).
    expect(removed).not.toContain(CUSTOM_ID);
    expect(fatals).toEqual([]);
    const lastWarn = warn.mock.calls[warn.mock.calls.length - 1];
    expect(lastWarn?.[0]).toContain('self-disabled (gl-error-0x502)');

    // A second self-disable before the restore runs is coalesced (re-entrancy).
    layer?.onSelfDisable?.('gl-error-0x502');
    expect(holder.scheduled).toBe(1);

    expect(holder.fn).not.toBeNull();
    holder.fn?.();
    // Error state: dead layer removed, onFatal fired once, stock NOT restored
    // (stays hidden from the clean enable - no fallback).
    expect(removed).toEqual([CUSTOM_ID]); // torn down exactly once
    expect(fatals).toEqual(['gl-error-0x502']);
    // Fatal is latched: a later sync() rebuilds nothing.
    c.sync(makeParams());
    expect(removed).toEqual([CUSTOM_ID]);
  });

  it('sync hook prefers a fresh-geometry resync over the stale snapshot', () => {
    // Regression (#customRender): replaying the cached `lastParams` after a
    // project switch stranded the custom layer on the previous project's
    // geometry (store 502 shots, custom stuck at 420). The hook must call the
    // injected resync (which reads current store geometry) and NOT replay.
    const { map } = makeFakeMap();
    const resync = vi.fn();
    const c = new SurveyCustomController(map, undefined, { resync });
    c.sync(makeParams()); // populates lastParams with this snapshot
    const syncSpy = vi.spyOn(c, 'sync');
    (c as unknown as { runSyncHook(): void }).runSyncHook();
    expect(resync).toHaveBeenCalledOnce();
    expect(syncSpy).not.toHaveBeenCalled();
  });

  it('sync hook falls back to the cached snapshot when no resync is wired', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    const params = makeParams();
    c.sync(params);
    const syncSpy = vi.spyOn(c, 'sync');
    (c as unknown as { runSyncHook(): void }).runSyncHook();
    expect(syncSpy).toHaveBeenCalledOnce();
    expect(syncSpy.mock.calls[0]?.[0]).toBe(params);
  });

  it('applies stored visibility toggles to the layer, including on first enable', () => {
    vi.spyOn(console, 'info').mockImplementation(() => {});
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    // Toggles set BEFORE the first sync must be applied to the fresh layer so an
    // enable never silently shows geometry the user hid.
    c.setShotsVisible(false);
    c.setStationsVisible(false);
    c.setReferenceShotsVisible(false);
    c.sync(makeParams());
    const layer = h.layerState.last;
    expect(layer).not.toBeNull();
    expect(layer?.shotsVisible).toBe(false);
    expect(layer?.stationsVisible).toBe(false);
    expect(layer?.referenceShotsVisible).toBe(false);

    // Later toggles reach the live layer directly.
    c.setShotsVisible(true);
    expect(layer?.shotsVisible).toBe(true);
  });

  it('is unconditional: stays active across re-syncs (no gate)', () => {
    vi.spyOn(console, 'info').mockImplementation(() => {});
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);
    // A second sync is a refresh (update()), never a disable - the custom layer
    // is the sole renderer.
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);
  });

  it('destroy() removes the custom layer (no stock to restore)', () => {
    vi.spyOn(console, 'info').mockImplementation(() => {});
    const { map, removed } = makeFakeMap();
    const c = new SurveyCustomController(map);
    c.sync(makeParams());
    expect(c.isActive()).toBe(true);
    c.destroy();
    expect(c.isActive()).toBe(false);
    expect(removed).toContain(CUSTOM_ID);
  });
});

describe('SurveyCustomController dispose (unmount-lite hook + cache release)', () => {
  // The controller installs its window sync hook only when `window` is defined;
  // the node test env has none, so stub a bare window per test and restore it.
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

  it('dispose() removes the window sync hook and a later sync() is inert', () => {
    const win = (globalThis as unknown as { window: Record<string, unknown> }).window;
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    c.sync(makeParams());
    const layer = h.layerState.last!;
    expect(typeof win[CUSTOM_RENDER_SYNC_HOOK]).toBe('function'); // hook installed
    const updatesBefore = layer.updateCalls;

    c.dispose();
    expect(win[CUSTOM_RENDER_SYNC_HOOK]).toBeUndefined(); // hook removed
    // dispose() must NOT touch the map (map.remove() handles GL teardown), and a
    // later sync() rebuilds nothing.
    c.sync(makeParams());
    expect(layer.updateCalls).toBe(updatesBefore); // inert: disposed guard bails
  });
});

describe('SurveyCustomController onActiveChange (stock-suppression signal)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.disabledReason = 'program-link-failed';
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('fires true once when the custom layer starts drawing; a re-sync does not re-fire', () => {
    const calls: boolean[] = [];
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map, undefined, { onActiveChange: (a) => calls.push(a) });
    c.sync(makeParams());
    expect(calls).toEqual([true]);
    // A re-sync is a refresh, not a transition - must NOT re-fire.
    c.sync(makeParams());
    expect(calls).toEqual([true]);
  });

  it('does NOT fire true when the custom layer fails to initialize', () => {
    h.layerState.disabledAfterAdd = true;
    h.layerState.disabledReason = 'webgl2-unavailable';
    const calls: boolean[] = [];
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map, undefined, { onActiveChange: (a) => calls.push(a) });
    c.sync(makeParams());
    // active never became true, so no suppression signal fires.
    expect(calls).toEqual([]);
  });

  it('does NOT fire false on a fatal self-disable (must NOT republish stock)', () => {
    // The fatal path removes the layer WITHOUT firing onActiveChange(false):
    // firing it would drive SurveyLayer to republish the stock bulk sources,
    // exactly the fallback this stage removes. The survey stays blank.
    const holder: { fn: (() => void) | null } = { fn: null };
    const calls: boolean[] = [];
    const fatals: string[] = [];
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map, undefined, {
      onActiveChange: (a) => calls.push(a),
      onFatal: (r) => fatals.push(r),
      scheduleRestore: (fn) => { holder.fn = fn; },
    });
    c.sync(makeParams());
    expect(calls).toEqual([true]);
    h.layerState.last?.onSelfDisable?.('gl-error-0x502');
    holder.fn?.();
    // onFatal fired; onActiveChange stays at [true] (no stock republish).
    expect(fatals).toEqual(['gl-error-0x502']);
    expect(calls).toEqual([true]);
  });

  it('fires false on destroy() when active (non-fatal teardown republishes stock)', () => {
    const calls: boolean[] = [];
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map, undefined, { onActiveChange: (a) => calls.push(a) });
    c.sync(makeParams());
    c.destroy();
    expect(calls).toEqual([true, false]);
  });

  it('a fatal self-disable latches: a later sync() does not re-enable', () => {
    // Under the sole-renderer contract a fatal GL fault is terminal: the layer is
    // removed and the controller latches `fatal`, so any later geometry flush
    // (sync) is a no-op rather than a flip-flop that would re-create the layer
    // that just failed. Ordering guard: the layer is removed exactly once.
    const holder: { fn: (() => void) | null } = { fn: null };
    const calls: boolean[] = [];
    const fatals: string[] = [];
    const { map, removed } = makeFakeMap();
    const c = new SurveyCustomController(map, undefined, {
      onActiveChange: (a) => calls.push(a),
      onFatal: (r) => fatals.push(r),
      scheduleRestore: (fn) => { holder.fn = fn; },
    });
    c.sync(makeParams());
    expect(calls).toEqual([true]);
    h.layerState.last?.onSelfDisable?.('gl-error-0x502');
    holder.fn?.(); // deferred fatal teardown
    expect(fatals).toEqual(['gl-error-0x502']);
    expect(removed).toEqual([CUSTOM_ID]);
    // Re-entrant / later sync is inert (fatal latch), never re-enabling.
    c.sync(makeParams());
    expect(removed).toEqual([CUSTOM_ID]);
    expect(fatals).toEqual(['gl-error-0x502']); // onFatal fires at most once
    expect(calls).toEqual([true]);
  });
});

describe('SurveyCustomController color-only fast path', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  // Shared geometry + themeColors refs so only the toggled color global differs
  // (mirrors production, where this.themeColors is a stable instance field).
  const scenario = () => {
    const themeColors = makeParams().colorCtx.themeColors;
    const surveyData = makeFixture();
    const base = { gpsOrigin: null, shotWidthPx: 2, stationRadiusPx: 3, stationStrokePx: 1 };
    const ctx = (over: Record<string, unknown>): CustomSyncParams['colorCtx'] =>
      ({ showDepthGradient: false, showIssueSeverity: false, depthRange: { min: 0, max: 1 }, themeColors, ...over }) as unknown as CustomSyncParams['colorCtx'];
    return { surveyData, base, ctx };
  };

  it('a pure color-global change re-uploads colors, no rebuild or layer re-create', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    const { surveyData, base, ctx } = scenario();
    c.sync({ ...base, surveyData, colorCtx: ctx({}) });
    const layer = h.layerState.last!;
    expect(layer.updateColorsCalls).toBe(0);
    // Toggle depth gradient ON: same geometry + themeColors -> color-only path.
    c.sync({ ...base, surveyData, colorCtx: ctx({ showDepthGradient: true }) });
    expect(h.layerState.last).toBe(layer); // no re-create
    expect(layer.updateColorsCalls).toBe(1); // colors re-uploaded
    expect(layer.updateCalls).toBe(0); // NOT a full rebuild
    expect(layer.uploadCalls).toBe(0); // NOT the geometry-incremental path
  });

  it('a geometry edit with stable colors still takes the incremental path', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    const { surveyData, base, ctx } = scenario();
    const color = ctx({});
    c.sync({ ...base, surveyData, colorCtx: color });
    const layer = h.layerState.last!;
    // Move a station (same colorCtx ref) -> geometry incremental, not color-only.
    const moved = makeFixture();
    moved.stationCoords[3] = 55; // station 1's x
    c.sync({ ...base, surveyData: moved, colorCtx: color });
    expect(layer.updateColorsCalls).toBe(0);
    expect(layer.updateCalls).toBe(0);
    expect(layer.uploadCalls).toBeGreaterThan(0);
  });

  it('a colour-scale preview re-uploads colours only, and the next sync replaces it exactly', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    const { surveyData, base, ctx } = scenario();
    const committed = ctx({ showDepthGradient: true });
    c.sync({ ...base, surveyData, colorCtx: committed });
    const layer = h.layerState.last!;
    // Preview: colours only - no rebuild, no geometry upload.
    expect(c.previewColors(ctx({ showDepthGradient: true }))).toBe(true);
    expect(layer.updateColorsCalls).toBe(1);
    expect(layer.updateCalls).toBe(0);
    expect(layer.uploadCalls).toBe(0);
    // The user reverts (hover, then Escape): the committed context comes back
    // unchanged, which would normally be a no-op - but the buffers hold the
    // preview's approximate colours, so the controller recolours exactly.
    c.sync({ ...base, surveyData, colorCtx: committed });
    expect(layer.updateColorsCalls).toBe(2);
    expect(layer.updateCalls).toBe(0);
    // ...and only once: a further identical sync is a no-op again.
    c.sync({ ...base, surveyData, colorCtx: committed });
    expect(layer.updateColorsCalls).toBe(2);
  });

  it('a preview before the layer is live is refused (the caller syncs instead)', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    const { ctx } = scenario();
    expect(c.previewColors(ctx({ showDepthGradient: true }))).toBe(false);
  });

  it('a combined geometry + color change falls back to a full rebuild', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    const { surveyData, base, ctx } = scenario();
    c.sync({ ...base, surveyData, colorCtx: ctx({}) });
    const layer = h.layerState.last!;
    const moved = makeFixture();
    moved.stationCoords[3] = 55;
    c.sync({ ...base, surveyData: moved, colorCtx: ctx({ showDepthGradient: true }) });
    expect(layer.updateColorsCalls).toBe(0);
    expect(layer.uploadCalls).toBe(0);
    expect(layer.updateCalls).toBe(1); // full rebuild via update()
  });
});

describe('SurveyCustomController highlight width/radius resolution (C2)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  const dataOf = (): CustomRenderData => h.layerState.last!.data as CustomRenderData;

  it('falls back to the baked stock ratios when no config sizes are passed', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    c.sync(makeParams()); // shotWidthPx 2, stationRadiusPx 3, no hover/selection px
    const d = dataOf();
    expect(d.lineHoverWidthMul).toBe(2); // default 4/2
    expect(d.lineSelectWidthMul).toBe(1.5); // default 3/2
    expect(d.circleHoverSizeMul).toBe(1.5); // default 6/4
    expect(d.circleSelectSizeMul).toBe(1.25); // default 5/4
  });

  it('derives the ratios from configured absolute px (independent of base)', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    c.sync({
      ...makeParams(),
      shotWidthPx: 2,
      shotHoverWidthPx: 5, // 5/2 = 2.5x
      shotSelectionWidthPx: 3, // 3/2 = 1.5x
      stationRadiusPx: 4,
      stationHoverRadiusPx: 10, // 10/4 = 2.5x
      stationSelectionRadiusPx: 6, // 6/4 = 1.5x
    });
    const d = dataOf();
    expect(d.lineHoverWidthMul).toBeCloseTo(2.5);
    expect(d.lineSelectWidthMul).toBeCloseTo(1.5);
    expect(d.circleHoverSizeMul).toBeCloseTo(2.5);
    expect(d.circleSelectSizeMul).toBeCloseTo(1.5);
  });
});

/**
 * Plan-mode idle tint (#395 revision): `buildCustomDimState`'s `planIdleDim`
 * output. The per-vertex GAS_DIM_LEVEL/anchored-wins-over-idle precedence itself
 * is exercised through `colorResolvers.test.ts` (color) and the gas-planning e2e
 * spec (the full renderer, incl. this dim path).
 */
describe('buildCustomDimState plan-idle tint (#395 revision)', () => {
  const noSets = new Set<number>();

  it('carries planIdleDim through untouched when no gas-reach mode is active', () => {
    const off = buildCustomDimState(noSets, noSets, noSets, false, null, false);
    expect(off.planIdleDim).toBe(false);
    const on = buildCustomDimState(noSets, noSets, noSets, false, null, true);
    expect(on.planIdleDim).toBe(true);
  });

  it('defaults planIdleDim to false when the caller omits it', () => {
    const state = buildCustomDimState(noSets, noSets, noSets, false, null);
    expect(state.planIdleDim).toBe(false);
  });
});

/**
 * The per-vertex dim VALUES the plan-idle tint actually produces
 * (controller.ts `dimForStation`/`dimForShot`: `gasDim || planIdleDim ?
 * GAS_DIM_LEVEL : 0`). The `buildCustomDimState` suite above only pins the flag
 * pass-through; this drives a real `sync()` + `setDimming()` so removing the
 * `|| planIdleDim` read fails on the built `circle.dim` buffer, not just on the
 * state object.
 */
describe('SurveyCustomController plan-idle tint dim buffer (#395 revision)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  const dataOf = (): CustomRenderData => h.layerState.last!.data as CustomRenderData;
  const noSets = new Set<number>();

  it('dims an anchored section fully (1) and everything else at GAS_DIM_LEVEL when idle', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    c.sync(makeParams());
    // Fixture: section 1 owns stations 0-2 (rows 0-2), section 2 owns station 0
    // (row 3). Anchor section 1; the idle tint should still reach section 2's
    // un-anchored station.
    c.setDimming(buildCustomDimState(new Set([1]), noSets, noSets, false, null, true));
    const dim = dataOf().circle.dim;
    // 4 vertices (corners) per station.
    for (let i = 0; i < 3 * 4; i++) expect(dim[i]).toBe(1);
    for (let i = 3 * 4; i < 4 * 4; i++) expect(dim[i]).toBe(GAS_DIM_LEVEL);
  });

  it('dims nothing when planIdleDim is false and no other dim source is set', () => {
    const { map } = makeFakeMap();
    const c = new SurveyCustomController(map);
    c.sync(makeParams());
    c.setDimming(buildCustomDimState(noSets, noSets, noSets, false, null, false));
    const dim = dataOf().circle.dim;
    for (let i = 0; i < dim.length; i++) expect(dim[i]).toBe(0);
  });
});
