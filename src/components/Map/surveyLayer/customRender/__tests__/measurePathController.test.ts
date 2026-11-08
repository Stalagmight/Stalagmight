import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import { MeasurePathController, type MeasurePathLayerLike } from '../measurePathController';
import { buildMeasurePathBuffers } from '../measurePathTessellate';
import type { MeasurePathRenderData } from '../surveyMeasurePathLayer';

/**
 * Measure-path controller state transitions (#26, stage 2a). The GL layer is a
 * scriptable fake (no WebGL), so we drive: add-at-z-order, update-in-place,
 * clear, empty-path clear, add-time disable (stock tint remains), and a
 * mid-render self-disable whose deferred removal is generation-guarded.
 */

/** Build a render-data payload with `segCount` shots + `ptCount` stations. */
function renderData(segCount: number, ptCount: number): MeasurePathRenderData {
  const segments = Array.from({ length: segCount }, (_, i) => ({
    a: [i, 0] as [number, number],
    b: [i + 1, 0] as [number, number],
  }));
  const points = Array.from({ length: ptCount }, (_, i) => ({ center: [i, 1] as [number, number] }));
  const { anchorX, anchorY, line, circle } = buildMeasurePathBuffers(segments, points);
  return {
    anchorX,
    anchorY,
    line,
    circle,
    lineHalfWidthPx: 1,
    circleRadiusPx: 2,
    aaPx: 1,
    color: [0.13, 0.83, 0.93, 1],
  };
}

/** Minimal scriptable fake of the measure-path GL layer. */
class FakeLayer implements MeasurePathLayerLike {
  readonly id: string;
  data: unknown;
  onSelfDisable: (reason: string) => void;
  private disabled: boolean;
  private reason: string | null;
  setDataCalls = 0;

  constructor(id: string, data: unknown, onSelfDisable: (reason: string) => void, disabled = false, reason: string | null = null) {
    this.id = id;
    this.data = data;
    this.onSelfDisable = onSelfDisable;
    this.disabled = disabled;
    this.reason = reason;
  }

  isDisabled(): boolean {
    return this.disabled;
  }
  getDisabledReason(): string | null {
    return this.reason;
  }
  setData(data: MeasurePathRenderData): void {
    this.data = data;
    this.setDataCalls++;
  }
}

/** A fake map recording addLayer/removeLayer/getLayer + a settable layer set. */
function makeFakeMap(existing: string[] = []) {
  const present = new Set(existing);
  const calls: { added: { id: string; beforeId: string | undefined }[]; removed: string[]; repaints: number } = {
    added: [],
    removed: [],
    repaints: 0,
  };
  const map = {
    getLayer: (id: string) => (present.has(id) ? ({ id } as unknown) : undefined),
    addLayer: (layer: { id: string }, beforeId?: string) => {
      present.add(layer.id);
      calls.added.push({ id: layer.id, beforeId });
    },
    removeLayer: (id: string) => {
      present.delete(id);
      calls.removed.push(id);
    },
    triggerRepaint: () => {
      calls.repaints++;
    },
  };
  return { map: map as unknown as MaplibreMap, calls, present };
}

describe('MeasurePathController', () => {
  let warn: ReturnType<typeof vi.spyOn>;
  beforeEach(() => {
    warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => {
    warn.mockRestore();
  });

  it('adds the layer at the first existing beforeId candidate', () => {
    const { map, calls } = makeFakeMap(['survey-stations']);
    const made: FakeLayer[] = [];
    const ctrl = new MeasurePathController(map, {
      makeLayer: (id, data, cb) => {
        const l = new FakeLayer(id, data, cb);
        made.push(l);
        return l;
      },
    });

    ctrl.setPath(renderData(2, 1), ['survey-interact-shots-measured', 'survey-stations', 'survey-measure-path']);

    expect(ctrl.isActive()).toBe(true);
    expect(calls.added).toHaveLength(1);
    // First candidate absent, second present -> insert before it.
    expect(calls.added[0]!.beforeId).toBe('survey-stations');
    expect(calls.repaints).toBeGreaterThan(0);
  });

  it('inserts with no beforeId when no candidate exists (draws on top)', () => {
    const { map, calls } = makeFakeMap([]);
    const ctrl = new MeasurePathController(map, { makeLayer: (id, d, cb) => new FakeLayer(id, d, cb) });
    ctrl.setPath(renderData(1, 0), ['nope-a', 'nope-b']);
    expect(calls.added[0]!.beforeId).toBeUndefined();
  });

  it('updates data in place on a second setPath (no re-add)', () => {
    const { map, calls } = makeFakeMap([]);
    let layer: FakeLayer | null = null;
    const ctrl = new MeasurePathController(map, {
      makeLayer: (id, d, cb) => (layer = new FakeLayer(id, d, cb)),
    });

    ctrl.setPath(renderData(1, 1), []);
    ctrl.setPath(renderData(3, 2), []);

    expect(calls.added).toHaveLength(1);
    expect(layer!.setDataCalls).toBe(1);
  });

  it('clear() frees the GL resources of a layer a worker restart already dropped', () => {
    const { map, calls, present } = makeFakeMap([]);
    let onRemoveCalls = 0;
    const ctrl = new MeasurePathController(map, {
      makeLayer: (id, d, cb) => Object.assign(new FakeLayer(id, d, cb), { onRemove: () => onRemoveCalls++ }),
    });
    ctrl.setPath(renderData(1, 1), []);
    present.clear();
    ctrl.clear();
    expect(calls.removed).toEqual([]);
    expect(onRemoveCalls).toBe(1);
  });

  it('clears the layer on clear()', () => {
    const { map, calls } = makeFakeMap([]);
    const ctrl = new MeasurePathController(map, { makeLayer: (id, d, cb) => new FakeLayer(id, d, cb) });
    ctrl.setPath(renderData(1, 1), []);
    ctrl.clear();
    expect(ctrl.isActive()).toBe(false);
    expect(calls.removed).toContain('survey-measure-path-custom');
  });

  it('treats an empty path (no shots + no stations) as a clear', () => {
    const { map, calls } = makeFakeMap([]);
    const ctrl = new MeasurePathController(map, { makeLayer: (id, d, cb) => new FakeLayer(id, d, cb) });
    ctrl.setPath(renderData(2, 1), []);
    expect(ctrl.isActive()).toBe(true);

    ctrl.setPath(renderData(0, 0), []);
    expect(ctrl.isActive()).toBe(false);
    expect(calls.removed).toContain('survey-measure-path-custom');

    // Passing null also clears.
    ctrl.setPath(renderData(2, 1), []);
    ctrl.setPath(null, []);
    expect(ctrl.isActive()).toBe(false);
  });

  it('removes a layer that disabled itself on add and surfaces onFatal (no stock fallback)', () => {
    const { map, calls } = makeFakeMap([]);
    const fatals: string[] = [];
    const ctrl = new MeasurePathController(map, {
      makeLayer: (id, d, cb) => new FakeLayer(id, d, cb, true, 'program-link-failed'),
      onFatal: (r) => fatals.push(r),
    });

    ctrl.setPath(renderData(1, 1), []);

    expect(ctrl.isActive()).toBe(false);
    expect(calls.removed).toContain('survey-measure-path-custom');
    expect(warn).toHaveBeenCalled();
    // Stock tint is gone, so the init failure must reach the app (subsystem-tagged).
    expect(fatals).toEqual(['measure-path: program-link-failed']);
  });

  it('surfaces onFatal on a deferred mid-render self-disable, at most once', () => {
    const { map } = makeFakeMap([]);
    const scheduled: (() => void)[] = [];
    const fatals: string[] = [];
    let layer: FakeLayer | null = null;
    const ctrl = new MeasurePathController(map, {
      makeLayer: (id, d, cb) => (layer = new FakeLayer(id, d, cb)),
      scheduleRestore: (fn) => scheduled.push(fn),
      onFatal: (r) => fatals.push(r),
    });

    ctrl.setPath(renderData(1, 1), []);
    layer!.onSelfDisable('gl-error-0x502');
    expect(fatals).toEqual([]); // deferred, not fired inside the render pass
    scheduled[0]!();
    expect(fatals).toEqual(['measure-path: gl-error-0x502']);
  });

  it('defers removal on a mid-render self-disable, generation-guarded', () => {
    const { map, calls } = makeFakeMap([]);
    const scheduled: (() => void)[] = [];
    let layer: FakeLayer | null = null;
    const ctrl = new MeasurePathController(map, {
      makeLayer: (id, d, cb) => (layer = new FakeLayer(id, d, cb)),
      scheduleRestore: (fn) => scheduled.push(fn),
    });

    ctrl.setPath(renderData(1, 1), []);
    expect(ctrl.isActive()).toBe(true);

    // The layer hits a GL error inside render() and reports it.
    layer!.onSelfDisable('gl-error-0x502');
    // Not removed synchronously (unsafe inside the render pass); deferred.
    expect(calls.removed).not.toContain('survey-measure-path-custom');
    expect(scheduled).toHaveLength(1);

    scheduled[0]!();
    expect(calls.removed).toContain('survey-measure-path-custom');
    expect(ctrl.isActive()).toBe(false);
  });

  it('no-ops a stale deferred self-disable removal after a newer clear/path', () => {
    const { map, calls } = makeFakeMap([]);
    const scheduled: (() => void)[] = [];
    let layer: FakeLayer | null = null;
    const ctrl = new MeasurePathController(map, {
      makeLayer: (id, d, cb) => (layer = new FakeLayer(id, d, cb)),
      scheduleRestore: (fn) => scheduled.push(fn),
    });

    ctrl.setPath(renderData(1, 1), []);
    const stale = layer!;
    stale.onSelfDisable('gl-error-0x502');
    // A newer path supersedes the drag generation before the deferred fires.
    ctrl.clear();
    calls.removed.length = 0;
    ctrl.setPath(renderData(2, 1), []);

    // The stale deferred removal must not tear down the fresh layer.
    scheduled[0]!();
    expect(calls.removed).not.toContain('survey-measure-path-custom');
    expect(ctrl.isActive()).toBe(true);
  });
});

/**
 * drawnCounts() - the observable E2E asserts on. The measure tint is a custom GL
 * layer with no MapLibre feature-state, so "is the path tinted" is answered by
 * how many segments/discs were handed to the layer, not by reading feature-state
 * off a (deleted) vector-tile source.
 */
describe('MeasurePathController.drawnCounts', () => {
  it('reports zero before any path, the counts while drawn, and zero after clear', () => {
    const { map } = makeFakeMap([]);
    const ctrl = new MeasurePathController(map, {
      makeLayer: (id, d, cb) => new FakeLayer(id, d, cb),
    });
    expect(ctrl.drawnCounts()).toEqual({ shots: 0, stations: 0 });

    ctrl.setPath(renderData(3, 4), []);
    expect(ctrl.drawnCounts()).toEqual({ shots: 3, stations: 4 });

    // A later update replaces the counts.
    ctrl.setPath(renderData(1, 2), []);
    expect(ctrl.drawnCounts()).toEqual({ shots: 1, stations: 2 });

    ctrl.clear();
    expect(ctrl.drawnCounts()).toEqual({ shots: 0, stations: 0 });
  });

  it('reports zero when the layer failed to initialize', () => {
    const { map } = makeFakeMap([]);
    const ctrl = new MeasurePathController(map, {
      makeLayer: (id, d, cb) => new FakeLayer(id, d, cb, true, 'program-link-failed'),
    });
    ctrl.setPath(renderData(2, 2), []);
    expect(ctrl.drawnCounts()).toEqual({ shots: 0, stations: 0 });
  });
});
