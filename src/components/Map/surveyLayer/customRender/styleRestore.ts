/**
 * Custom layers across MapLibre style rebuilds (#606).
 *
 * MapLibre 6 rebuilds the whole style from `style.serialize()` on two paths:
 * a WebGL context loss + restore (`Map._contextLost` / `_contextRestored`), and a
 * worker restart (`Map._handleWorkersRestarted`, which `watchWorkers()` triggers
 * on `visibilitychange -> visible` when the workers miss a ping). Custom layers
 * cannot be serialized, so both paths drop every one of them. Both fire
 * `webglcontextlost` and later `webglcontextrestored`, while the rebuilt style
 * loads one frame after that (`setStyle` -> `loadJSON` -> `frameAsync`) and
 * announces it with `style.load`.
 *
 * Between the loss and that `style.load` no layer can be added: the style is null
 * during a context loss (`addLayer` would lazily build an empty style and run
 * `onAdd` against the dead context) and not yet loaded after the rebuild
 * (`addLayer` throws "Style is not done loading"). {@link addCustomLayer} holds
 * a layer back in that window, and the owner's reattach adds it once the style
 * has loaded.
 *
 * The two paths also differ in what they leave behind: a context loss destroys
 * the old style, which runs each custom layer's `onRemove`; a worker restart
 * drops the old style without calling it, so the layer still owns live GL
 * objects. {@link reAddCustomLayer} and {@link removeCustomLayer} free them
 * (our layers' `onRemove` is idempotent).
 */
import type { CustomLayerInterface, Map as MaplibreMap } from 'maplibre-gl';

/** A custom layer that can report a failed `onAdd`. */
export interface RestorableCustomLayer extends CustomLayerInterface {
  isDisabled(): boolean;
}

interface RestoreTracker {
  /** Between a loss and the `style.load` of the rebuilt style. */
  restoring: boolean;
  awaitingLoad: boolean;
  readonly subscribers: Set<() => void>;
}

const trackers = new WeakMap<MaplibreMap, RestoreTracker>();

function trackerFor(map: MaplibreMap): RestoreTracker {
  const existing = trackers.get(map);
  if (existing) return existing;
  const tracker: RestoreTracker = { restoring: false, awaitingLoad: false, subscribers: new Set() };
  const onLoad = (): void => {
    tracker.awaitingLoad = false;
    tracker.restoring = false;
    for (const fn of [...tracker.subscribers]) fn();
  };
  map.on('webglcontextlost', () => {
    tracker.restoring = true;
  });
  map.on('webglcontextrestored', () => {
    tracker.restoring = true;
    if (tracker.awaitingLoad) return;
    tracker.awaitingLoad = true;
    map.once('style.load', onLoad);
  });
  trackers.set(map, tracker);
  return tracker;
}

/**
 * Run `reattach` once the style rebuilt by a context restore or a worker restart
 * has loaded. Returns an unsubscribe.
 */
export function onStyleRestored(map: MaplibreMap, reattach: () => void): () => void {
  const tracker = trackerFor(map);
  tracker.subscribers.add(reattach);
  return () => {
    tracker.subscribers.delete(reattach);
  };
}

/** True while no layer can be added: a restore is under way, or the context is
 *  lost and MapLibre has not dispatched the loss yet. */
export function isStyleRestoring(map: MaplibreMap): boolean {
  if (trackers.get(map)?.restoring) return true;
  return liveGl(map)?.isContextLost?.() === true;
}

function liveGl(map: MaplibreMap): WebGL2RenderingContext | undefined {
  return (map as { painter?: { context?: { gl?: WebGL2RenderingContext } } }).painter?.context?.gl;
}

/**
 * `map.addLayer(layer, beforeId)`, unless a restore is under way: then the layer
 * stays pending (not in the style, not failed) and the owner's reattach adds it
 * through {@link reAddCustomLayer}.
 */
export function addCustomLayer(map: MaplibreMap, layer: CustomLayerInterface, beforeId?: string): void {
  if (!isStyleRestoring(map)) map.addLayer(layer, beforeId);
}

/**
 * Remove `layer` from the style, or, when a rebuild already dropped it (or it
 * was never added), free its GL resources directly so a worker restart cannot
 * leak them.
 */
export function removeCustomLayer(
  map: MaplibreMap,
  layer: Pick<CustomLayerInterface, 'id' | 'onRemove'> | null,
): void {
  if (!layer) return;
  if (map.getLayer?.(layer.id)) map.removeLayer(layer.id);
  else layer.onRemove?.(map, liveGl(map) as WebGL2RenderingContext);
}

/**
 * Add `layer` back to the rebuilt style with fresh GL resources. No-op when the
 * style still holds it. Returns false when the layer could not start again.
 */
export function reAddCustomLayer(
  map: MaplibreMap,
  layer: RestorableCustomLayer,
  beforeId?: string,
): boolean {
  if (map.getLayer(layer.id)) return true;
  layer.onRemove?.(map, liveGl(map) as WebGL2RenderingContext);
  map.addLayer(layer, beforeId);
  return !layer.isDisabled();
}
