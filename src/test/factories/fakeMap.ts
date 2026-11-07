/**
 * A recording MapLibre stand-in for the controller/layer unit tests (#576 E15):
 * the superset of the nine `makeFakeMap` copies those tests used to carry.
 *
 * The map is a plain object (no Proxy), so a member it lacks - one it never
 * defines, or one `omit` leaves out - is absent, and calling it throws. It models
 * what the controllers touch: the layers on the map with their paint and layout
 * visibility, GeoJSON sources, `on`/`off` listeners, repaints; and it records
 * every mutation for the test to assert on. The richer lifecycle fake for
 * `new SurveyLayer(map)` (style loss, `remove()`) is
 * `src/components/Map/__tests__/surveyLayerFakeMap.ts`.
 *
 * Synthetic: no MapLibre code runs.
 */
import { vi, type Mock } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';

type Listener = (e: unknown) => void;

/** The members the fake map defines (and `omit` can leave out). */
export type FakeMapMember =
  | 'style'
  | 'getStyle'
  | 'getLayer'
  | 'addLayer'
  | 'removeLayer'
  | 'getSource'
  | 'addSource'
  | 'removeSource'
  | 'isSourceLoaded'
  | 'setPaintProperty'
  | 'getLayoutProperty'
  | 'setLayoutProperty'
  | 'on'
  | 'off'
  | 'project'
  | 'triggerRepaint';

export interface FakeMapOptions {
  /** Layer ids already on the map (e.g. the stock layers a controller hides), bottom to top. */
  layers?: Iterable<string>;
  /** Seeded layout `visibility` per layer id (an id seeded here is on the map too). */
  visibility?: Record<string, string | undefined>;
  /**
   * Map members to leave out, for code that probes for them (`map.on?.(...)`,
   * `hasLiveStyle(map)`): `['style']` is a stand-in without a live style,
   * `['on', 'off']` one nothing can subscribe to. The map has every member
   * otherwise, `style` (a live style, `{}`) included.
   */
  omit?: readonly FakeMapMember[];
  /**
   * What `addLayer` does with a custom layer's `onAdd`: 'map' (default) calls
   * `onAdd(map, {})`; 'bare' calls `onAdd(undefined, undefined)` (no map, no GL
   * context); 'skip' never calls it.
   */
  onAdd?: 'map' | 'bare' | 'skip';
  /**
   * Give the map a DOM canvas container (appended to `document.body`, jsdom only)
   * plus the drag state the release forwarder reads: `dragPan.isActive()` answers
   * `dragActive` (default true), and `_handlers.handleEvent` is present unless
   * `withHandlers` is false.
   */
  dom?: FakeMapDomOptions;
}

export interface FakeMapDomOptions {
  dragActive?: boolean;
  withHandlers?: boolean;
}

export interface PaintCall {
  layer: string;
  prop: string;
  value: unknown;
}

export interface FakeMap {
  map: MaplibreMap;
  /** Ids of the layers on the map now. */
  layers: Set<string>;
  /** Paint per layer on the map: the `addLayer` spec's paint, updated by `setPaintProperty`. */
  paint: Map<string, Record<string, unknown>>;
  /** Ids of the sources on the map now. */
  sources: Set<string>;
  /** The id of every `addLayer` call, in call order. */
  added: string[];
  /** Every `addLayer` call with the `beforeId` it passed. */
  addCalls: Array<{ id: string; beforeId: string | undefined }>;
  /** The id of every `removeLayer` call, in call order. */
  removed: string[];
  /** Layout `visibility` per layer id: the seed, then every `setLayoutProperty(id, 'visibility', v)`. */
  visibility: Record<string, string | undefined>;
  /** Every `setPaintProperty` call, in order (a layer not on the map included). */
  paintCalls: PaintCall[];
  /** How many times a source's `setData` ran. */
  readonly setDataCount: number;
  /** How many times `triggerRepaint` ran. */
  readonly repaintCount: number;
  /**
   * The listeners `on(type, fn)` registered and `off` has not removed, in
   * registration order. Like MapLibre's `Evented`, registering the same function
   * twice keeps one entry.
   */
  listeners(type: string): Listener[];
  /** `listeners(type).length`. */
  listenerCount(type: string): number;
  /** Call every listener of `type` with `event`. */
  emit(type: string, event?: unknown): void;
  /** Take a layer off the map behind the code's back (not recorded in `removed`). */
  dropLayer(id: string): void;
}

/** What a `dom` fake adds: the canvas container, the canvas inside it, and `_handlers.handleEvent`. */
export interface FakeMapDom {
  canvasContainer: HTMLDivElement;
  canvas: HTMLCanvasElement;
  handleEvent: Mock;
}

export function makeFakeMap(options: FakeMapOptions & { dom: FakeMapDomOptions }): FakeMap & FakeMapDom;
export function makeFakeMap(options?: FakeMapOptions): FakeMap;
export function makeFakeMap(options: FakeMapOptions = {}): FakeMap {
  const visibility: Record<string, string | undefined> = { ...(options.visibility ?? {}) };
  const layers = new Set<string>([...(options.layers ?? []), ...Object.keys(visibility)]);
  const paint = new Map<string, Record<string, unknown>>();
  const sources = new Set<string>();
  const added: string[] = [];
  const addCalls: Array<{ id: string; beforeId: string | undefined }> = [];
  const removed: string[] = [];
  const paintCalls: PaintCall[] = [];
  const registered = new Map<string, Listener[]>();
  const counts = { setData: 0, repaint: 0 };
  const onAdd = options.onAdd ?? 'map';

  const map: Record<string, unknown> = {
    style: {},
    getStyle: () => ({}),
    getLayer: (id: string) => (layers.has(id) ? { id } : undefined),
    addLayer: (
      layer: { id: string; paint?: Record<string, unknown>; onAdd?: (m: unknown, gl: unknown) => void },
      beforeId?: string,
    ) => {
      layers.add(layer.id);
      paint.set(layer.id, { ...(layer.paint ?? {}) });
      added.push(layer.id);
      addCalls.push({ id: layer.id, beforeId });
      if (onAdd === 'map') layer.onAdd?.(map, {});
      else if (onAdd === 'bare') layer.onAdd?.(undefined, undefined);
    },
    removeLayer: (id: string) => {
      layers.delete(id);
      paint.delete(id);
      removed.push(id);
    },
    getSource: (id: string) =>
      sources.has(id)
        ? {
            setData: () => {
              counts.setData++;
            },
          }
        : undefined,
    addSource: (id: string) => {
      sources.add(id);
    },
    removeSource: (id: string) => {
      sources.delete(id);
    },
    isSourceLoaded: () => true,
    setPaintProperty: (layer: string, prop: string, value: unknown) => {
      paintCalls.push({ layer, prop, value });
      const p = paint.get(layer);
      if (p) p[prop] = value;
    },
    getLayoutProperty: (id: string, prop: string) => (prop === 'visibility' ? visibility[id] : undefined),
    setLayoutProperty: (id: string, prop: string, value: unknown) => {
      if (prop === 'visibility') visibility[id] = value as string;
    },
    on: (type: string, fn: Listener) => {
      const list = registered.get(type) ?? [];
      if (!list.includes(fn)) registered.set(type, [...list, fn]);
    },
    off: (type: string, fn: Listener) => {
      const list = registered.get(type) ?? [];
      const i = list.indexOf(fn);
      if (i >= 0) registered.set(type, [...list.slice(0, i), ...list.slice(i + 1)]);
    },
    project: ([lng, lat]: [number, number]) => ({ x: lng, y: lat }),
    triggerRepaint: () => {
      counts.repaint++;
    },
  };
  for (const member of options.omit ?? []) delete map[member];

  const fake: FakeMap = {
    map: map as unknown as MaplibreMap,
    layers,
    paint,
    sources,
    added,
    addCalls,
    removed,
    visibility,
    paintCalls,
    get setDataCount() {
      return counts.setData;
    },
    get repaintCount() {
      return counts.repaint;
    },
    listeners: (type) => [...(registered.get(type) ?? [])],
    listenerCount: (type) => registered.get(type)?.length ?? 0,
    emit: (type, event = {}) => {
      for (const fn of [...(registered.get(type) ?? [])]) fn(event);
    },
    dropLayer: (id) => {
      layers.delete(id);
    },
  };

  if (options.dom) {
    const { dragActive = true, withHandlers = true } = options.dom;
    const canvasContainer = document.createElement('div');
    const canvas = document.createElement('canvas');
    canvasContainer.appendChild(canvas);
    document.body.appendChild(canvasContainer);
    const handleEvent = vi.fn();
    map.getCanvasContainer = () => canvasContainer;
    map.dragPan = { isActive: () => dragActive };
    if (withHandlers) map._handlers = { handleEvent };
    const dom: FakeMapDom = { canvasContainer, canvas, handleEvent };
    return Object.assign(fake, dom);
  }
  return fake;
}
