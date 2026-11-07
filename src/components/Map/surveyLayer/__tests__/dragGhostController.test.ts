import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DRAG_PREVIEW_SOURCE } from '../dragPreviewLayers';
import type { DragPreviewSet } from '../dragPreviewSource';
import { dropStyle } from '../customRender/__tests__/styleGone';
import { makeFakeMap } from '../../../../test/factories/fakeMap';

/**
 * Drag-ghost controller (#mem drag latency). Locks the behaviour that replaces
 * the async ghost-overshoot pipeline:
 *   - the CUSTOM path drives the preview with a single shader-uniform write +
 *     triggerRepaint and touches NO GeoJSON source / worker (the overshoot fix);
 *   - it falls back to the STOCK GeoJSON preview per-drag when the custom layer
 *     cannot initialize, and mid-drag when it self-disables on a GL error, so the
 *     user is never left without a preview.
 *
 * The custom ghost GL layer is mocked (no WebGL in node); the stock path runs the
 * REAL dragPreviewLayers + GhostFollow against a fake map that records sources,
 * layers, setData, and sourcedata listeners.
 */
const h = vi.hoisted(() => {
  const layerState = { disabledAfterAdd: false, last: null as FakeGhostLayer | null };

  class FakeGhostLayer {
    readonly id: string;
    readonly type = 'custom' as const;
    onSelfDisable: ((reason: string) => void) | null;
    private disabled: boolean;
    setOffsetCalls: Array<[number, number]> = [];

    constructor(id: string, _data: unknown, onSelfDisable: ((reason: string) => void) | null = null) {
      this.id = id;
      this.onSelfDisable = onSelfDisable;
      this.disabled = layerState.disabledAfterAdd;
      layerState.last = this;
    }
    isDisabled(): boolean {
      return this.disabled;
    }
    getDisabledReason(): string | null {
      return this.disabled ? 'program-link-failed' : null;
    }
    setOffset(dx: number, dy: number): void {
      this.setOffsetCalls.push([dx, dy]);
    }
  }

  return { layerState, FakeGhostLayer };
});

vi.mock('../customRender/surveyGhostLayer', () => ({ SurveyGhostLayer: h.FakeGhostLayer }));

import { DragGhostController, type DragGhostOptions } from '../dragGhostController';
import type { GhostRenderData } from '../customRender/surveyGhostLayer';

const GHOST_ID = 'survey-drag-ghost';

const SET: DragPreviewSet = {
  features: [
    { emit: 'point', coords: [[0, 0]], movingEnd: 'uniform', properties: { kind: 'station', color: '#00ff00' } },
    { emit: 'line', coords: [[0, 0], [1, 1]], movingEnd: 'uniform', properties: { kind: 'shot', color: '#ff0000' } },
  ],
};

const STUB_DATA = { anchorX: 0, anchorY: 0 } as unknown as GhostRenderData;

function makeOptions(over: Partial<DragGhostOptions> = {}): DragGhostOptions {
  return {
    ghostData: () => STUB_DATA,
    layerCtx: () => ({ colorExpr: ['get', 'color'], shotWidth: 3, stationRadius: 5, selectHighlight: '#fff' }),
    projector: () => ({ project: ([lng, lat]) => ({ x: lng, y: lat }), unproject: ([x, y]) => ({ lng: x, lat: y }) }),
    dpr: () => 2,
    // Synchronous restore so the self-disable fallback is observable in-test.
    scheduleRestore: (fn) => fn(),
    ...over,
  };
}

beforeEach(() => {
  h.layerState.disabledAfterAdd = false;
  h.layerState.last = null;
});
afterEach(() => vi.clearAllMocks());

describe('custom path is the default for every drag (bulk render mode agnostic)', () => {
  it('goes custom whenever the layer can init - the controller has no bulk-mode gate', () => {
    // There is no customActive input: custom is chosen for every drag that can
    // initialize, so the default profile (custom labels + STOCK geometry) gets
    // the overshoot-free custom ghost too. Only init failure / self-disable
    // (below) fall back to the stock GeoJSON ghost.
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(fake.map, makeOptions());
    expect(ctrl.begin(SET)).toBe('custom');
    expect(fake.layers.has(GHOST_ID)).toBe(true);
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(false);
  });
});

describe('custom path: synchronous, no GeoJSON source / worker', () => {
  it('begins custom, drives the offset by uniform, and never touches the stock preview', () => {
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(fake.map, makeOptions());
    expect(ctrl.begin(SET)).toBe('custom');
    expect(ctrl.isCustom()).toBe(true);
    expect(fake.layers.has(GHOST_ID)).toBe(true);
    // No stock ghost source / layers created, no setData.
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(false);
    expect(fake.setDataCount).toBe(0);

    ctrl.move(30, -12);
    // dpr=2 -> device px offset; one repaint, still no setData/source.
    const calls = h.layerState.last!.setOffsetCalls;
    expect(calls[calls.length - 1]).toEqual([60, -24]);
    expect(fake.repaintCount).toBeGreaterThan(0);
    expect(fake.setDataCount).toBe(0);
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(false);

    ctrl.end();
    expect(fake.layers.has(GHOST_ID)).toBe(false);
  });
});

describe('per-drag fallback: custom layer fails to initialize', () => {
  it('drops the dead layer and uses the stock GeoJSON preview', () => {
    h.layerState.disabledAfterAdd = true;
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(fake.map, makeOptions());
    expect(ctrl.begin(SET)).toBe('stock');
    expect(ctrl.isCustom()).toBe(false);
    // Dead custom layer removed; stock source created + one bake@0 setData.
    expect(fake.layers.has(GHOST_ID)).toBe(false);
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(true);
    expect(fake.setDataCount).toBe(1);

    // A move past the rebake threshold, once the @0 bake lands, re-bakes (setData).
    ctrl.move(200, 0);
    fake.listeners('sourcedata').forEach((fn) => fn({ sourceId: DRAG_PREVIEW_SOURCE }));
    expect(fake.setDataCount).toBeGreaterThan(1);
    expect(fake.paintCalls.length).toBeGreaterThan(0);
    ctrl.end();
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(false);
  });
});

describe('mid-drag self-disable falls back to stock at the live offset', () => {
  it('removes the disabled custom layer and spins up the stock ghost', () => {
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(fake.map, makeOptions());
    expect(ctrl.begin(SET)).toBe('custom');
    ctrl.move(120, 40); // live offset the fallback must replay
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(false);

    // The GL layer self-disables mid-render; scheduleRestore runs synchronously.
    h.layerState.last!.onSelfDisable!('gl-error-0x505');
    expect(ctrl.isCustom()).toBe(false);
    expect(fake.layers.has(GHOST_ID)).toBe(false);
    // Stock ghost now standing in, baked (the @0 start bake + the live-offset replay).
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(true);
    expect(fake.setDataCount).toBeGreaterThan(0);
    ctrl.end();
  });
});

describe('a deferred self-disable restore is scoped to the drag that scheduled it', () => {
  it('a stale drag-1 restore does not tear down drag-2 healthy custom ghost', () => {
    const holder: { fn: (() => void) | null } = { fn: null };
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(
      fake.map,
      makeOptions({ scheduleRestore: (fn) => { holder.fn = fn; } }),
    );
    // Drag 1: custom, then a mid-drag self-disable schedules a restore (deferred).
    expect(ctrl.begin(SET)).toBe('custom');
    h.layerState.last!.onSelfDisable!('gl-error-0x505');
    expect(holder.fn).not.toBeNull();

    // Drag 1 ends; drag 2 begins fresh and healthy on the custom path.
    ctrl.end();
    expect(ctrl.begin(SET)).toBe('custom');
    expect(fake.layers.has(GHOST_ID)).toBe(true);

    // The stale drag-1 restore fires late: it must no-op (generation mismatch),
    // leaving drag 2 on its custom ghost with no stock fallback spun up.
    holder.fn!();
    expect(ctrl.isCustom()).toBe(true);
    expect(fake.layers.has(GHOST_ID)).toBe(true);
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(false);
    ctrl.end();
  });

  it("drag-2's own self-disable still fires after a prior drag self-disabled", () => {
    const holder: { fns: Array<() => void> } = { fns: [] };
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(
      fake.map,
      makeOptions({ scheduleRestore: (fn) => { holder.fns.push(fn); } }),
    );
    expect(ctrl.begin(SET)).toBe('custom');
    h.layerState.last!.onSelfDisable!('gl-error-1'); // drag 1 self-disable scheduled
    ctrl.end(); // clears the coalescing guard so drag 2 isn't blocked by a stale flag

    expect(ctrl.begin(SET)).toBe('custom');
    h.layerState.last!.onSelfDisable!('gl-error-2'); // drag 2's own self-disable
    holder.fns[holder.fns.length - 1]!(); // run drag 2's restore
    expect(ctrl.isCustom()).toBe(false);
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(true);
    ctrl.end();
  });
});

// #576 D16: the lightweight unmount releases the ghost without map work - the
// map is about to be removed with every layer on it.
describe('dispose (map-free teardown for the lightweight unmount)', () => {
  it('drops the stock bake timer and land listener without touching layers', () => {
    vi.useFakeTimers();
    try {
      h.layerState.disabledAfterAdd = true; // custom cannot init -> stock path
      const fake = makeFakeMap();
      const ctrl = new DragGhostController(fake.map, makeOptions());
      expect(ctrl.begin(SET)).toBe('stock');
      expect(vi.getTimerCount()).toBe(1); // the bake-landed fallback
      expect(fake.listeners('sourcedata')).toHaveLength(1);
      const layersBefore = new Set(fake.layers);
      const translates = fake.paintCalls.length;

      ctrl.dispose();

      expect(vi.getTimerCount()).toBe(0);
      expect(fake.listeners('sourcedata')).toHaveLength(0);
      expect(fake.layers).toEqual(layersBefore);
      expect(fake.paintCalls.length).toBe(translates);
      expect(ctrl.isCustom()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('invalidates a pending self-disable restore', () => {
    const holder: { fn: (() => void) | null } = { fn: null };
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(fake.map, makeOptions({ scheduleRestore: (fn) => { holder.fn = fn; } }));
    expect(ctrl.begin(SET)).toBe('custom');
    h.layerState.last!.onSelfDisable!('gl-error-1');
    ctrl.dispose();
    holder.fn!();
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(false);
  });
});

describe('a carry cancelled while the WebGL context is lost (#576 NF-MAP-5)', () => {
  it('end() does not read the missing style', () => {
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(fake.map, makeOptions());
    expect(ctrl.begin(SET)).toBe('custom');
    dropStyle(fake.map);
    expect(() => ctrl.end()).not.toThrow();
    expect(ctrl.isCustom()).toBe(false);
  });
});

describe('a context loss between a self-disable and its deferred fallback (#576 RV4-FE-MF-6)', () => {
  it('the fallback does not read the missing style; the restore re-runs it on a live style', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    const fake = makeFakeMap();
    const scheduled: (() => void)[] = [];
    const ctrl = new DragGhostController(fake.map, makeOptions({ scheduleRestore: (fn) => scheduled.push(fn) }));
    expect(ctrl.begin(SET)).toBe('custom');
    ctrl.move(30, 10);

    // A genuine GL error disables the ghost mid-render, then the context goes
    // before the deferred fallback runs.
    const ghost = h.layerState.last!;
    (ghost as unknown as { disabled: boolean }).disabled = true;
    ghost.onSelfDisable!('gl-error-0x505');
    const m = fake.map as unknown as Record<string, unknown>;
    const live = { style: m.style, getLayer: m.getLayer, addLayer: m.addLayer, removeLayer: m.removeLayer };
    dropStyle(fake.map);
    expect(() => scheduled[0]!()).not.toThrow();
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(false);

    // The restore: MapLibre has a style again, and glContextRecovery re-adds the
    // custom ghost, which is still disabled - so the fallback runs now, for real.
    Object.assign(m, live);
    fake.layers.delete(GHOST_ID);
    ctrl.reattach(undefined);
    expect(scheduled).toHaveLength(2);
    scheduled[1]!();
    expect(ctrl.isCustom()).toBe(false);
    expect(fake.layers.has(GHOST_ID)).toBe(false);
    expect(fake.sources.has(DRAG_PREVIEW_SOURCE)).toBe(true);
    ctrl.end();
  });
});

describe('a STOCK-ghost carry while the WebGL context is lost (#576 RV4-FE-SL2)', () => {
  it('move() and end() do not read the missing style', () => {
    // The custom ghost cannot initialize, so the carry runs on the stock ghost.
    h.layerState.disabledAfterAdd = true;
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(fake.map, makeOptions());
    expect(ctrl.begin(SET)).toBe('stock');
    const setDataBefore = fake.setDataCount;

    // The loss: every style-backed call, sources and paint included, throws.
    dropStyle(fake.map);
    const m = fake.map as unknown as Record<string, unknown>;
    const thrower = (name: string) => () => {
      throw new TypeError(`Cannot read properties of null (reading '${name}')`);
    };
    m.getSource = thrower('getSource');
    m.removeSource = thrower('removeSource');
    m.setPaintProperty = thrower('setPaintProperty');

    let threw: unknown = null;
    try {
      // A small move (translate only), the @0 bake landing, then a move past the
      // rebake threshold (a bake), then the cancel.
      ctrl.move(2, 1);
      fake.listeners('sourcedata').forEach((fn) => fn({ sourceId: DRAG_PREVIEW_SOURCE }));
      ctrl.move(200, 0);
      ctrl.end();
    } catch (e) {
      threw = e;
    }
    console.log('[SL2] stock carry during a loss threw:', threw ? String(threw) : 'no');
    expect(threw).toBeNull();
    expect(ctrl.isCustom()).toBe(false);
    // Nothing was pushed into a source the loss took away.
    expect(fake.setDataCount).toBe(setDataBefore);
  });
});

describe('a pick-up while the WebGL context is lost (#576 NF-W4MAP-1)', () => {
  it('begins idle: nothing thrown, no layer or source added, and the next pick-up draws again', () => {
    const fake = makeFakeMap();
    const ctrl = new DragGhostController(fake.map, makeOptions());
    dropStyle(fake.map);
    // Every style-backed call MapLibre makes reads the missing style, sources and
    // paint included (getSource / addSource / setPaintProperty -> this.style.*).
    const m = fake.map as unknown as Record<string, unknown>;
    const thrower = (name: string) => () => {
      throw new TypeError(`Cannot read properties of null (reading '${name}')`);
    };
    const addLayer = vi.fn(m.addLayer as () => void);
    m.addLayer = addLayer;
    m.getSource = thrower('getSource');
    m.addSource = thrower('addSource');
    m.setPaintProperty = thrower('setPaintProperty');

    expect(() => ctrl.begin(SET)).not.toThrow();
    // Not even attempted: MapLibre's addLayer lazily builds an EMPTY style when
    // it has none (`_lazyInitEmptyStyle`), which a lost context must not get.
    expect(addLayer).not.toHaveBeenCalled();
    expect(fake.layers.size).toBe(0);
    expect(fake.sources.size).toBe(0);
    expect(ctrl.isCustom()).toBe(false);
    expect(() => ctrl.move(4, 2)).not.toThrow();
    expect(() => ctrl.end()).not.toThrow();

    // The restore brings a live style back; the next pick-up builds its ghost.
    const live = makeFakeMap();
    const next = new DragGhostController(live.map, makeOptions());
    expect(next.begin(SET)).toBe('custom');
    expect(live.layers.has(GHOST_ID)).toBe(true);
  });
});
