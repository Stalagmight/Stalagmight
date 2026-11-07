/**
 * A MapLibre stand-in rich enough for `new SurveyLayer(map)` under node, for the
 * lifecycle tests (#576). Every style-backed call goes through `this.style`, and
 * `remove()` deletes it (Map.remove() -> setStyle(null) does exactly that), so a
 * call that lands after the map is gone throws the "Cannot read properties of
 * undefined" TypeError the app logged. `loseStyle()` / `restoreStyle()` model a
 * WebGL context loss, during which MapLibre also has no style (it sets
 * `this.style = null`); `restoreStyleLoading()` / `finishStyleLoad()` model the
 * frame after the restore, when the rebuilt style is in place but refuses writes.
 *
 * Stricter than maplibre-gl 6.16 on purpose: there `getLayer`, `getSource`,
 * `setPaintProperty` and `setFilter` read the style with `?.` and so survive a
 * missing style, while `addLayer`, `removeLayer`, `addSource`, `removeSource` and
 * `setLayoutProperty` still throw. Here they all throw, so a test catches a call
 * that reaches a missing style at all, not only the calls that crash today.
 *
 * Anything the tests do not care about is a no-op: the map is a Proxy whose
 * unknown members resolve to a function returning undefined.
 */
import type { Map as MaplibreMap } from 'maplibre-gl';

type Listener = (e: unknown) => void;

/** EventTarget that also counts its live listeners per type. */
export class CountingTarget extends EventTarget {
  private readonly live = new Map<string, Set<unknown>>();
  style = { cursor: '' };
  width = 800;
  height = 600;
  clientWidth = 800;
  clientHeight = 600;

  override addEventListener(type: string, fn: EventListenerOrEventListenerObject | null, opts?: boolean | AddEventListenerOptions): void {
    if (fn) {
      const set = this.live.get(type) ?? new Set();
      set.add(fn);
      this.live.set(type, set);
    }
    super.addEventListener(type, fn, opts);
  }

  override removeEventListener(type: string, fn: EventListenerOrEventListenerObject | null, opts?: boolean | EventListenerOptions): void {
    if (fn) this.live.get(type)?.delete(fn);
    super.removeEventListener(type, fn, opts);
  }

  listenerCount(type: string): number {
    return this.live.get(type)?.size ?? 0;
  }

  getBoundingClientRect(): { left: number; top: number; width: number; height: number } {
    return { left: 0, top: 0, width: this.width, height: this.height };
  }
}

interface FakeStyle {
  layers: string[];
  sources: Set<string>;
  layout: Map<string, string | undefined>;
  /** `${layerId}|${property}` -> the last value set. */
  paint: Map<string, unknown>;
  /** layerId -> the filter `setFilter` last set (serialized with the style, as MapLibre does). */
  filters: Map<string, unknown>;
  /** sourceId -> the data the last `setData` pushed (a GeoJSON source serializes its data). */
  data: Map<string, unknown>;
}

export interface FakeSurveyMap {
  map: MaplibreMap;
  canvas: CountingTarget;
  container: CountingTarget;
  /** Fire a map event to every registered listener. */
  fire(type: string, event?: unknown): void;
  listenerCount(type: string): number;
  /** Layer ids bottom to top (only meaningful while the style is alive). */
  layerOrder(): string[];
  /** Map.remove(): the style is deleted. */
  remove(): void;
  /** WebGL context loss: MapLibre drops the style (custom layers included). */
  loseStyle(): void;
  /** Context restore: the serialized style (custom layers excluded) comes back. */
  restoreStyle(): void;
  /**
   * Context restore as MapLibre 6 really does it: `setStyle` puts a NEW style in
   * place that loads one frame later. Until {@link finishStyleLoad} it is empty
   * (`getLayer` / `getSource` find nothing, `getLayersOrder` is `[]`) and refuses
   * every write with "Style is not done loading." (`Style._checkLoaded`).
   */
  restoreStyleLoading(): void;
  /** The frame after {@link restoreStyleLoading}: the serialized style is in place
   *  and accepts writes, and `style.load` fires. */
  finishStyleLoad(): void;
  /** True while the style is in place but has not loaded yet. */
  styleLoading(): boolean;
  /** Ids of the custom layers currently on the map. */
  customLayers(): string[];
  /** The last value `setPaintProperty` wrote for a layer's paint property. */
  paint(layerId: string, property: string): unknown;
  /** The errors of `addLayer` calls refused because their `beforeId` was not on the
   *  map - MapLibre fires an ErrorEvent and does NOT add the layer. */
  rejectedAdds(): string[];
  /** The filter a layer has on the live style (undefined when none or no style). */
  filter(layerId: string): unknown;
  /** The data the last `setData` pushed into a source on the live style. */
  sourceData(sourceId: string): unknown;
}

export function makeFakeSurveyMap(): FakeSurveyMap {
  const listeners = new Map<string, Set<Listener>>();
  const onceWrappers = new Map<Listener, Listener>();
  const canvas = new CountingTarget();
  const container = new CountingTarget();
  const custom = new Set<string>();
  const rejected: string[] = [];
  let serialized: FakeStyle | null = null;
  /** The style a {@link FakeSurveyMap.restoreStyleLoading} restore loads next frame. */
  let loading: FakeStyle | null = null;
  /** The last loss hit a style that had not loaded: there is no snapshot to restore. */
  let nothingSerialized = false;
  const takeNothingSerialized = (): boolean => {
    const was = nothingSerialized;
    nothingSerialized = false;
    return was;
  };

  const newStyle = (): FakeStyle => ({
    layers: [],
    sources: new Set(),
    layout: new Map(),
    paint: new Map(),
    filters: new Map(),
    data: new Map(),
  });

  const styleApi = (s: FakeStyle) => ({
    getLayer: (id: string) => (s.layers.includes(id) ? { id } : undefined),
    getSource: (id: string) =>
      s.sources.has(id) ? { type: 'geojson', setData: (d: unknown) => void s.data.set(id, d) } : undefined,
  });

  /** `_loaded` mirrors MapLibre's `Style._loaded`, the flag `_checkLoaded` reads. */
  type StyleRef = ReturnType<typeof styleApi> & { _s: FakeStyle; _loaded: boolean };
  /** The style a write lands on. Like MapLibre 6's `Style._checkLoaded`, a style
   *  that has not loaded yet refuses it. */
  const writable = (): FakeStyle => {
    const style = target.style!;
    if (!style._loaded) throw new Error('Style is not done loading.');
    return style._s;
  };
  const target = {
    // Optional so remove() can `delete` it, as MapLibre does.
    style: undefined as StyleRef | null | undefined,
    getLayer(id: string) {
      return target.style!.getLayer(id);
    },
    getSource(id: string) {
      return target.style!.getSource(id);
    },
    addSource(id: string) {
      writable().sources.add(id);
    },
    removeSource(id: string) {
      writable().sources.delete(id);
    },
    addLayer(layer: { id: string; type?: string; onAdd?: (m: unknown, gl: unknown) => void }, beforeId?: string) {
      const s = writable();
      const at = beforeId !== undefined ? s.layers.indexOf(beforeId) : -1;
      // Like MapLibre's Style.addLayer (maplibre-gl 5.16): a beforeId that is not
      // on the map is an ErrorEvent, and the layer is NOT added.
      if (beforeId !== undefined && at < 0) {
        rejected.push(`Cannot add layer "${layer.id}" before non-existing layer "${beforeId}".`);
        return;
      }
      if (at >= 0) s.layers.splice(at, 0, layer.id);
      else s.layers.push(layer.id);
      if (layer.type === 'custom') {
        custom.add(layer.id);
        layer.onAdd?.(proxy, {});
      }
    },
    removeLayer(id: string) {
      const s = writable();
      s.layers = s.layers.filter((l) => l !== id);
      custom.delete(id);
    },
    getLayersOrder() {
      return [...target.style!._s.layers];
    },
    getLayoutProperty(id: string) {
      return target.style!._s.layout.get(id);
    },
    setLayoutProperty(id: string, _name: string, value: string) {
      writable().layout.set(id, value);
    },
    setPaintProperty(id: string, name: string, value: unknown) {
      writable().paint.set(`${id}|${name}`, value);
    },
    getFilter(id: string) {
      return target.style!._s.filters.get(id);
    },
    setFilter(id: string, filter: unknown) {
      const f = writable().filters;
      if (filter === null || filter === undefined) f.delete(id);
      else f.set(id, filter);
    },
    on(type: string, fn: Listener) {
      const set = listeners.get(type) ?? new Set();
      set.add(fn);
      listeners.set(type, set);
    },
    // Like MapLibre, off() also removes a listener registered with once().
    once(type: string, fn: Listener) {
      const wrapped: Listener = (e) => {
        listeners.get(type)?.delete(wrapped);
        onceWrappers.delete(fn);
        fn(e);
      };
      onceWrappers.set(fn, wrapped);
      target.on(type, wrapped);
    },
    off(type: string, fn: Listener) {
      listeners.get(type)?.delete(fn);
      const wrapped = onceWrappers.get(fn);
      if (wrapped) listeners.get(type)?.delete(wrapped);
      onceWrappers.delete(fn);
    },
    getCanvas: () => canvas,
    getCanvasContainer: () => container,
    unproject: ([x, y]: [number, number]) => ({ lng: x, lat: y }),
    project: ([lng, lat]: [number, number]) => ({ x: lng, y: lat }),
    isSourceLoaded: () => true,
    isStyleLoaded: () => Boolean(target.style),
    getZoom: () => 18,
  };

  const s0 = newStyle();
  target.style = { ...styleApi(s0), _s: s0, _loaded: true };

  const proxy = new Proxy(target, {
    get(t, prop) {
      if (prop in t) return (t as Record<string | symbol, unknown>)[prop];
      return () => undefined;
    },
  }) as unknown as MaplibreMap;

  const fire = (type: string, event: unknown = {}): void => {
    for (const fn of [...(listeners.get(type) ?? [])]) fn(event);
  };

  return {
    map: proxy,
    canvas,
    container,
    fire,
    listenerCount: (type) => listeners.get(type)?.size ?? 0,
    layerOrder: () => [...(target.style?._s.layers ?? [])],
    remove: () => {
      delete (target as { style?: StyleRef | null }).style;
    },
    loseStyle: () => {
      const style = target.style!;
      const s = style._s;
      // A style that has not loaded serializes to nothing (MapLibre's
      // `Style.serialize` returns undefined), and destroying it aborts its load:
      // a loss inside a restore's loading frame leaves nothing to restore.
      nothingSerialized = !style._loaded;
      loading = null;
      serialized = nothingSerialized
        ? null
        : {
          layers: s.layers.filter((id) => !custom.has(id)),
          sources: new Set(s.sources),
          layout: new Map(s.layout),
          paint: new Map(s.paint),
          filters: new Map(s.filters),
          data: new Map(s.data),
        };
      custom.clear();
      target.style = null;
    },
    restoreStyle: () => {
      if (takeNothingSerialized()) return;
      const s = serialized ?? newStyle();
      serialized = null;
      target.style = { ...styleApi(s), _s: s, _loaded: true };
    },
    restoreStyleLoading: () => {
      // `_restoreStyleSnapshot` returns early without a snapshot: the map keeps no style.
      if (takeNothingSerialized()) return;
      loading = serialized ?? newStyle();
      serialized = null;
      const empty = newStyle();
      target.style = { ...styleApi(empty), _s: empty, _loaded: false };
    },
    finishStyleLoad: () => {
      if (!loading) return;
      const s = loading;
      loading = null;
      target.style = { ...styleApi(s), _s: s, _loaded: true };
      fire('style.load');
    },
    styleLoading: () => target.style?._loaded === false,
    customLayers: () => [...custom],
    paint: (layerId, property) => target.style?._s.paint.get(`${layerId}|${property}`),
    rejectedAdds: () => [...rejected],
    filter: (layerId) => target.style?._s.filters.get(layerId),
    sourceData: (sourceId) => target.style?._s.data.get(sourceId),
  };
}
