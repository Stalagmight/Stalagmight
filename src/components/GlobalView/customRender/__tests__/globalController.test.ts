/**
 * Controller tests for the global view's custom render (#customRender). The
 * custom layer is the SOLE global-survey renderer: no gate, no stock layers, no
 * stock fallback. The GL layer is a scriptable fake so the unconditional
 * activation, error-state routing (onFatal), active-state callback, and the
 * geometry-based pick can be exercised without a real WebGL context. The pure
 * buffer build + hit index run for real.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import { dropStyle } from '../../../Map/surveyLayer/customRender/__tests__/styleGone';
import { makeFakeMap } from '../../../../test/factories/fakeMap';
import type { ProjectMetadata } from '../../../../types/project';
import type { GeometryShot, SurveyGeometry } from '../../../../types/survey';
import type { SystemGeometry } from '../../systemColor';
import { GlobalCustomController } from '../globalController';
import { onStyleRestored } from '../../../Map/surveyLayer/customRender/styleRestore';
import type { ProjectFn } from '../globalHitIndex';

const h = vi.hoisted(() => {
  const layerState = {
    disabledAfterAdd: false,
    disabledReason: 'program-link-failed' as string,
    last: null as FakeLayer | null,
  };

  class FakeLayer {
    readonly id: string;
    onSelfDisable: ((reason: string) => void) | null;
    private disabled = false;
    private reason: string | null = null;
    hoverSystem = -1;
    visible = true;
    referenceShotsVisible = true;

    constructor(id: string, _data: unknown, onSelfDisable: ((reason: string) => void) | null = null) {
      this.id = id;
      this.onSelfDisable = onSelfDisable;
      layerState.last = this;
    }
    onAdd(): void {
      if (layerState.disabledAfterAdd) {
        this.disabled = true;
        this.reason = layerState.disabledReason;
      }
    }
    onRemoveCalls = 0;
    onRemove(): void {
      this.onRemoveCalls++;
    }
    isDisabled(): boolean {
      return this.disabled;
    }
    getDisabledReason(): string | null {
      return this.reason;
    }
    update(): void {}
    updateDim(): void {}
    updateSelected(): void {}
    setVisible(v: boolean): void {
      this.visible = v;
    }
    setReferenceShotsVisible(v: boolean): void {
      this.referenceShotsVisible = v;
    }
    setHoverSystem(i: number): void {
      this.hoverSystem = i;
    }
  }

  return { layerState, FakeLayer };
});

vi.mock('../globalCustomLayer', () => ({ GlobalCustomLayer: h.FakeLayer }));

const CUSTOM_ID = 'global-survey-custom';

/**
 * Fake map modelling MapLibre 6 across a style rebuild: during a context loss
 * the style is null and `addLayer` lazily builds one, running `onAdd` against
 * the dead context (the layer disables itself); the rebuilt style refuses
 * layers until `style.load` ("Style is not done loading"); the rebuild drops
 * every custom layer.
 */
function makeRestoringMap() {
  type Phase = 'loaded' | 'null' | 'rebuilt';
  let phase: Phase = 'loaded';
  const existing = new Set<string>();
  const added: string[] = [];
  const listeners: Array<{ type: string; fn: () => void; once: boolean }> = [];
  const fire = (type: string): void => {
    for (const l of listeners.filter((x) => x.type === type)) {
      if (l.once) listeners.splice(listeners.indexOf(l), 1);
      l.fn();
    }
  };
  const map = {
    painter: { context: { gl: {} } },
    getStyle: () => ({}),
    getLayer: (id: string) => (existing.has(id) ? { id } : undefined),
    addLayer: (layer: { id: string; onAdd?: (m: unknown, gl: unknown) => void }) => {
      if (phase === 'rebuilt') throw new Error('Style is not done loading.');
      const deadContext = phase === 'null';
      h.layerState.disabledAfterAdd = deadContext;
      existing.add(layer.id);
      added.push(layer.id);
      layer.onAdd?.(map, {});
      h.layerState.disabledAfterAdd = false;
    },
    removeLayer: (id: string) => existing.delete(id),
    on: (type: string, fn: () => void) => listeners.push({ type, fn, once: false }),
    once: (type: string, fn: () => void) => listeners.push({ type, fn, once: true }),
    off: () => {},
    project: ([lng, lat]: [number, number]) => ({ x: lng, y: lat }),
    triggerRepaint: () => {},
  } as unknown as MaplibreMap;
  const lose = (): void => {
    phase = 'null';
    existing.clear();
    fire('webglcontextlost');
  };
  const rebuild = (): void => {
    phase = 'rebuilt';
    existing.clear();
    fire('webglcontextrestored');
  };
  const load = (): void => {
    phase = 'loaded';
    fire('style.load');
  };
  return { map, added, lose, rebuild, load, restore: () => (rebuild(), load()) };
}

function shot(from: [number, number], to: [number, number], isReference = false): GeometryShot {
  return { from, to, kind: isReference ? ('Reference' as const) : ('Normal' as const), section_id: 0 };
}

function makeSystems(): SystemGeometry[] {
  const metadata = (fp: string): ProjectMetadata => ({
    name: fp,
    location: null,
    file_path: fp,
    modified_at: '2026-01-01T00:00:00Z',
    section_count: 1,
    total_length: 100,
    station_count: 2,
    gps_center: [0, 0],
    gps_bounds: null,
    is_remote: false,
  });
  const geometry = (shots: GeometryShot[]): SurveyGeometry => ({ shots, origin: [0, 0] });
  return [{ metadata: metadata('a.tml'), geometry: geometry([shot([0, 0], [10, 0])]) }];
}

const project: ProjectFn = ([lng, lat]) => ({ x: lng, y: lat });

describe('GlobalCustomController', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.disabledReason = 'program-link-failed';
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('enables the custom layer, fires onActiveChange(true), becomes active', () => {
    const { map, added, removed } = makeFakeMap();
    const active: boolean[] = [];
    const c = new GlobalCustomController(map, undefined, { onActiveChange: (a) => active.push(a) });
    c.sync(makeSystems());
    // Added at the top (sole renderer, no stock layers to hide).
    expect(added).toEqual([CUSTOM_ID]);
    expect(removed).not.toContain(CUSTOM_ID);
    expect(c.isActive()).toBe(true);
    expect(active).toEqual([true]);
  });

  it('is unconditional: a re-sync stays active, no re-fire', () => {
    const { map, removed } = makeFakeMap();
    const active: boolean[] = [];
    const c = new GlobalCustomController(map, undefined, { onActiveChange: (a) => active.push(a) });
    c.sync(makeSystems());
    // A second sync is a refresh (update()), never a disable - the custom layer
    // is the sole renderer, so it stays active.
    c.sync(makeSystems());
    expect(removed).not.toContain(CUSTOM_ID);
    expect(c.isActive()).toBe(true);
    expect(active).toEqual([true]); // true fires once, never re-fires
  });

  it('onAdd-disabled: fires onFatal, removes the dead layer, stays inactive, no fallback', () => {
    h.layerState.disabledAfterAdd = true;
    h.layerState.disabledReason = 'webgl2-unavailable';
    const { map, removed } = makeFakeMap();
    const active: boolean[] = [];
    const fatals: string[] = [];
    const c = new GlobalCustomController(map, undefined, {
      onActiveChange: (a) => active.push(a),
      onFatal: (r) => fatals.push(r),
    });
    c.sync(makeSystems());
    expect(removed).toContain(CUSTOM_ID);
    expect(c.isActive()).toBe(false);
    expect(active).toEqual([]); // never fired true
    expect(fatals).toEqual(['webgl2-unavailable']);
    // Fatal latch: a later sync() is inert.
    c.sync(makeSystems());
    expect(removed.filter((id) => id === CUSTOM_ID).length).toBe(1);
    expect(fatals).toEqual(['webgl2-unavailable']); // at most once
  });

  it('reattach re-adds the same layer after a style rebuild dropped it, freeing it first', () => {
    const { map, added, dropLayer } = makeFakeMap();
    const c = new GlobalCustomController(map);
    c.sync(makeSystems());
    const layer = h.layerState.last!;
    dropLayer(CUSTOM_ID); // the rebuilt style holds no custom layers
    c.reattach();
    expect(added).toEqual([CUSTOM_ID, CUSTOM_ID]);
    expect(h.layerState.last).toBe(layer);
    expect(layer.onRemoveCalls).toBe(1);
    expect(c.isActive()).toBe(true);
    c.reattach(); // still in the style: no-op
    expect(added).toEqual([CUSTOM_ID, CUSTOM_ID]);
  });

  it('reattach that cannot start again routes to onFatal', () => {
    const { map, dropLayer, removed } = makeFakeMap();
    const fatals: string[] = [];
    const c = new GlobalCustomController(map, undefined, { onFatal: (r) => fatals.push(r) });
    c.sync(makeSystems());
    dropLayer(CUSTOM_ID);
    h.layerState.disabledAfterAdd = true;
    c.reattach();
    expect(removed).toContain(CUSTOM_ID);
    expect(fatals).toEqual(['program-link-failed']);
    expect(c.isActive()).toBe(false);
  });

  it('a controller started while the style is null (context lost) draws after the restore', () => {
    const m = makeRestoringMap();
    const fatals: string[] = [];
    const c = new GlobalCustomController(m.map, undefined, { onFatal: (r) => fatals.push(r) });
    onStyleRestored(m.map, () => c.reattach());
    m.lose();
    c.sync(makeSystems());
    m.restore();
    expect(m.added).toEqual([CUSTOM_ID]);
    expect(h.layerState.last!.isDisabled()).toBe(false);
    expect(c.isActive()).toBe(true);
    expect(fatals).toEqual([]);
  });

  it('a controller started before the rebuilt style loads draws after it does', () => {
    const m = makeRestoringMap();
    const c = new GlobalCustomController(m.map);
    onStyleRestored(m.map, () => c.reattach());
    m.lose();
    m.rebuild();
    expect(() => c.sync(makeSystems())).not.toThrow();
    m.load();
    expect(m.added).toEqual([CUSTOM_ID]);
    expect(c.isActive()).toBe(true);
  });

  it('teardown frees the GL resources of a layer a worker restart already dropped', () => {
    const { map, dropLayer, removed } = makeFakeMap();
    const c = new GlobalCustomController(map);
    c.sync(makeSystems());
    const layer = h.layerState.last!;
    dropLayer(CUSTOM_ID);
    c.destroy();
    expect(removed).toEqual([]);
    expect(layer.onRemoveCalls).toBe(1);
  });

  it('self-disable fires onFatal (deferred), removes the layer, no stock republish', () => {
    const holder: { fn: (() => void) | null } = { fn: null };
    const { map, removed } = makeFakeMap();
    const active: boolean[] = [];
    const fatals: string[] = [];
    const c = new GlobalCustomController(map, undefined, {
      scheduleRestore: (fn) => (holder.fn = fn),
      onActiveChange: (a) => active.push(a),
      onFatal: (r) => fatals.push(r),
    });
    c.sync(makeSystems());
    // Simulate a GL error mid-render.
    h.layerState.last!.onSelfDisable?.('gl-error-0x505');
    // Deferred: nothing torn down until the scheduled callback runs.
    expect(removed).not.toContain(CUSTOM_ID);
    holder.fn?.();
    // Error state: dead layer removed, onFatal fired. The fatal path sets active
    // directly, so onActiveChange never sees a trailing false (no republish).
    expect(removed).toContain(CUSTOM_ID);
    expect(fatals).toEqual(['gl-error-0x505']);
    expect(active).toEqual([true]); // no false -> no fallback signal
    expect(c.isActive()).toBe(false);
  });

  it('pick resolves a system only while active + visible', () => {
    const { map } = makeFakeMap();
    const c = new GlobalCustomController(map);
    // Inactive -> null.
    expect(c.pick(project, 0, 0)).toBeNull();
    c.sync(makeSystems());
    expect(c.pick(project, 0, 0)?.systemId).toBe('a.tml');
    // Markers mode hides the survey -> no hits.
    c.setVisible(false);
    expect(c.pick(project, 0, 0)).toBeNull();
  });

  it('destroy() while active removes the custom layer and goes inactive', () => {
    const { map, removed } = makeFakeMap();
    const c = new GlobalCustomController(map);
    c.sync(makeSystems());
    expect(c.isActive()).toBe(true);
    c.destroy();
    expect(c.isActive()).toBe(false);
    expect(removed).toEqual([CUSTOM_ID]);
    // A later sync() after destroy re-enables cleanly (destroy is not fatal).
    c.sync(makeSystems());
    expect(c.isActive()).toBe(true);
  });

  it('setHover maps a system id to its baked index (or -1)', () => {
    const { map } = makeFakeMap();
    const c = new GlobalCustomController(map);
    c.sync(makeSystems());
    c.setHover('a.tml');
    expect(h.layerState.last!.hoverSystem).toBe(0);
    c.setHover(null);
    expect(h.layerState.last!.hoverSystem).toBe(-1);
    c.setHover('missing.tml');
    expect(h.layerState.last!.hoverSystem).toBe(-1);
  });
});

describe('GlobalCustomController deferred fatal teardown without a style (#576 NF-MAP-3)', () => {
  beforeEach(() => {
    h.layerState.disabledAfterAdd = false;
    h.layerState.last = null;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it('a context loss before the deferred teardown: no throw, still latches fatal', () => {
    const holder: { fn: (() => void) | null } = { fn: null };
    const fatals: string[] = [];
    const { map } = makeFakeMap();
    const c = new GlobalCustomController(map, undefined, {
      scheduleRestore: (fn) => (holder.fn = fn),
      onFatal: (r) => fatals.push(r),
    });
    c.sync(makeSystems());
    h.layerState.last!.onSelfDisable?.('gl-error-0x505');
    dropStyle(map);
    expect(() => holder.fn?.()).not.toThrow();
    expect(fatals).toEqual(['gl-error-0x505']);
    expect(c.isActive()).toBe(false);
  });
});
