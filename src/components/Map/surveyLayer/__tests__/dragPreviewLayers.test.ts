import { describe, it, expect } from 'vitest';
import type * as maplibregl from 'maplibre-gl';
import {
  DRAG_PREVIEW_SOURCE,
  createDragPreviewLayers,
  removeDragPreviewLayers,
  setDragPreviewTranslate,
  type DragPreviewLayerCtx,
} from '../dragPreviewLayers';

/**
 * Ghost-follow latency fix (#mem drag latency). The moving preview ("ghost")
 * used to update ONLY through a GeoJSON `setData` -> MapLibre worker parse +
 * retile round-trip, landing several frames after the pointermove for a large
 * moving set - so it lagged the cursor and, notably, lagged MORE than the tiny
 * interact-overlay highlight (which re-tiles in ~1 frame). The fix keeps the
 * per-frame re-`setData` but adds a SYNCHRONOUS, viewport-anchored `*-translate`
 * that tracks the cursor with no worker in the loop. These tests lock the
 * translate path: viewport anchor (screen-px), a plain synchronous
 * `setPaintProperty` (no source/`setData`/worker touch), and the uniform offset
 * across all three preview layers (shots + closures share it with stations, so
 * they cannot skew apart).
 */

interface PaintCall {
  layer: string;
  prop: string;
  value: unknown;
}

interface FakeMap {
  map: maplibregl.Map;
  layers: Map<string, { paint: Record<string, unknown> }>;
  paintCalls: PaintCall[];
  sourceDataCalls: number;
}

function makeFakeMap(): FakeMap {
  const layers = new Map<string, { paint: Record<string, unknown> }>();
  const sources = new Set<string>();
  const paintCalls: PaintCall[] = [];
  let sourceDataCalls = 0;
  const map = {
    getSource: (id: string) => (sources.has(id) ? { setData: () => { sourceDataCalls++; } } : undefined),
    addSource: (id: string) => { sources.add(id); },
    removeSource: (id: string) => { sources.delete(id); },
    getLayer: (id: string) => (layers.has(id) ? { id } : undefined),
    addLayer: (spec: { id: string; paint?: Record<string, unknown> }) => {
      layers.set(spec.id, { paint: { ...(spec.paint ?? {}) } });
    },
    removeLayer: (id: string) => { layers.delete(id); },
    setPaintProperty: (layer: string, prop: string, value: unknown) => {
      paintCalls.push({ layer, prop, value });
      const l = layers.get(layer);
      if (l) l.paint[prop] = value;
    },
  } as unknown as maplibregl.Map;
  return {
    map,
    layers,
    paintCalls,
    get sourceDataCalls() {
      return sourceDataCalls;
    },
  } as unknown as FakeMap;
}

const CTX: DragPreviewLayerCtx = {
  colorExpr: ['get', 'color'] as unknown as DragPreviewLayerCtx['colorExpr'],
  shotWidth: 4,
  stationRadius: 6,
  selectHighlight: '#ff0000',
};

const SHOTS = 'survey-drag-preview-shots';
const CLOSURES = 'survey-drag-preview-closures';
const STATIONS = 'survey-drag-preview-stations';

describe('createDragPreviewLayers translate anchor', () => {
  it('sets a viewport (screen-px) translate anchor on every preview layer', () => {
    const f = makeFakeMap();
    createDragPreviewLayers(f.map, CTX);
    expect(f.layers.get(SHOTS)!.paint['line-translate-anchor']).toBe('viewport');
    expect(f.layers.get(CLOSURES)!.paint['line-translate-anchor']).toBe('viewport');
    expect(f.layers.get(STATIONS)!.paint['circle-translate-anchor']).toBe('viewport');
  });
});

describe('setDragPreviewTranslate', () => {
  it('applies the SAME offset uniformly to shots, closures, and stations', () => {
    const f = makeFakeMap();
    createDragPreviewLayers(f.map, CTX);
    setDragPreviewTranslate(f.map, [12, -7]);
    expect(f.layers.get(SHOTS)!.paint['line-translate']).toEqual([12, -7]);
    expect(f.layers.get(CLOSURES)!.paint['line-translate']).toEqual([12, -7]);
    expect(f.layers.get(STATIONS)!.paint['circle-translate']).toEqual([12, -7]);
  });

  it('is a synchronous paint-property update with no source/setData/worker touch', () => {
    const f = makeFakeMap();
    createDragPreviewLayers(f.map, CTX);
    const before = f.sourceDataCalls;
    setDragPreviewTranslate(f.map, [3, 4]);
    // No async boundary: the ghost offset lands via setPaintProperty alone; the
    // GeoJSON source (worker round-trip) is never touched by the follow path.
    expect(f.sourceDataCalls).toBe(before);
    expect(f.paintCalls).toEqual([
      { layer: SHOTS, prop: 'line-translate', value: [3, 4] },
      { layer: CLOSURES, prop: 'line-translate', value: [3, 4] },
      { layer: STATIONS, prop: 'circle-translate', value: [3, 4] },
    ]);
  });

  it('no-ops for layers that do not exist (mid-teardown / never created)', () => {
    const f = makeFakeMap();
    // No createDragPreviewLayers: nothing to translate.
    expect(() => setDragPreviewTranslate(f.map, [5, 5])).not.toThrow();
    expect(f.paintCalls).toHaveLength(0);
    // After removal the helper is likewise inert.
    createDragPreviewLayers(f.map, CTX);
    removeDragPreviewLayers(f.map);
    f.paintCalls.length = 0;
    setDragPreviewTranslate(f.map, [5, 5]);
    expect(f.paintCalls).toHaveLength(0);
  });
});

describe('createDragPreviewLayers', () => {
  it('creates the source + all three layers, and removeDragPreviewLayers tears them down', () => {
    const f = makeFakeMap();
    createDragPreviewLayers(f.map, CTX);
    expect(f.layers.has(SHOTS)).toBe(true);
    expect(f.layers.has(CLOSURES)).toBe(true);
    expect(f.layers.has(STATIONS)).toBe(true);
    expect(f.map.getSource(DRAG_PREVIEW_SOURCE)).toBeDefined();
    removeDragPreviewLayers(f.map);
    expect(f.layers.size).toBe(0);
    expect(f.map.getSource(DRAG_PREVIEW_SOURCE)).toBeUndefined();
  });
});
