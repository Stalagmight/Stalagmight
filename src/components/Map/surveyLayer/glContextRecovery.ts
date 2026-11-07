/**
 * Bring the custom WebGL layers back after MapLibre rebuilt the style without
 * them (#576 D38, #606).
 *
 * MapLibre 6 rebuilds the style from `style.serialize()`, which skips custom
 * layers, on two paths (customRender/styleRestore.ts has the event sequence):
 *
 * - a WebGL context loss (a GPU reset, a driver crash, some sleep/resume cycles,
 *   WebKit memory pressure): on the canvas's `webglcontextlost` it serializes the
 *   style, destroys it (custom layers get `onRemove`) and, on
 *   `webglcontextrestored`, re-applies the copy;
 * - a worker restart (`_handleWorkersRestarted`, after the window becomes visible
 *   and a worker misses a ping): it serializes the style, fires a synthetic map
 *   `webglcontextlost` and `webglcontextrestored`, and swaps the copy in WITHOUT
 *   calling `onRemove`, so the dropped layers still own live GL objects.
 *
 * Either way the copy loads one frame later (`style.load`), and MapLibre never
 * re-adds a custom layer - it warns "cannot be restored after WebGL context loss.
 * You will need to re-add it manually" - so every survey layer (geometry, labels,
 * measured-path tint, drag ghost) and the Global View's survey stayed gone until
 * the map was remounted.
 *
 * This module re-adds them. The layer objects survive with their data (their
 * `onRemove` only drops GL handles), so re-adding the SAME object makes MapLibre
 * run its `onAdd` again, which recompiles the programs and re-uploads the
 * buffers from that data on the live context. Each controller re-adds through
 * `reAddCustomLayer`, which first frees what a worker restart left behind.
 *
 * Timing is styleRestore.ts's per-map tracker (one set of map listeners): the
 * re-add runs on the `style.load` that follows the restore. Until then
 * `isStyleRestoring` is true, so a layer a controller adds in that window is held
 * back (`addCustomLayer`) and goes back here with the others.
 *
 * Z-order: the layer order is snapshotted while the old style is still alive -
 * for a context loss in the capture phase on the canvas container, which runs
 * before MapLibre's own `webglcontextlost` listener on the canvas itself; for a
 * worker restart, which has no canvas event, on the map's `webglcontextlost`,
 * when the old style is still in place. Each custom layer is re-inserted before
 * the first layer that followed it then and is on the map now
 * ({@link reattachInOrder}); a layer that was not on the map at the snapshot
 * (held back during the rebuild) goes where a fresh add puts it.
 */
import type { Map as MaplibreMap } from 'maplibre-gl';
import { onStyleRestored } from './customRender/styleRestore';

/** A controller that owns one custom layer and can re-add it. */
export interface ReattachableLayer {
  /** The custom layer's id on the map. */
  readonly layerId: string;
  /**
   * Re-add the live layer object before `beforeId`, its successor in the
   * pre-rebuild order; undefined: where a fresh add puts it (none of its
   * successors is on the map, or it was not on the map at the snapshot). No-op
   * when the controller has nothing drawing.
   */
  reattach(beforeId?: string): void;
}

/**
 * Re-add the custom layers `owners` holds so the stack ends up as `order` (the
 * pre-loss order, bottom to top).
 *
 * Works top-down, and picks each layer's insertion point against the LIVE map
 * (`onMap`) at the moment it is re-added: the first layer that followed it in
 * `order` and is on the map now. Everything above it has been dealt with by then,
 * so that is its true successor - including when a custom layer above it did not
 * land (its controller had nothing to draw any more, or the re-add failed and it
 * tore itself down). Trusting the plan instead put the layer below before one that
 * is not on the map, which MapLibre refuses ("Cannot add layer ... before
 * non-existing layer"), so one failed re-add took the survey geometry with it
 * (#576 D38 review). Ids in `order` without an owner, and owners whose layer is not
 * in `order` (nothing drawing at the loss), are skipped.
 */
export function reattachInOrder(
  order: readonly string[],
  owners: ReadonlyMap<string, ReattachableLayer>,
  onMap: (id: string) => boolean,
): void {
  for (let i = order.length - 1; i >= 0; i--) {
    const owner = owners.get(order[i]!);
    if (!owner) continue;
    let beforeId: string | undefined;
    for (let j = i + 1; j < order.length; j++) {
      if (onMap(order[j]!)) {
        beforeId = order[j];
        break;
      }
    }
    owner.reattach(beforeId);
  }
}

export interface GlContextRecoveryOptions {
  /** The custom layers to bring back, bottom to top, read when the style is back. */
  layers: () => readonly ReattachableLayer[];
  /**
   * Puts right style state that should survive into the rebuilt copy. On a
   * context loss it runs just before MapLibre serializes and destroys the style,
   * while it is still alive. A worker restart serializes before it says anything,
   * so there it runs on the rebuilt style once it has loaded, before the layers go
   * back.
   */
  beforeLoss?: () => void;
  /** Runs once the custom layers are back: replay what changed meanwhile. */
  afterRestore: () => void;
}

/**
 * Watch `map` for a style rebuild (a WebGL context loss or a worker restart) and
 * restore the custom layers once the rebuilt style has loaded. Returns the detach
 * function.
 */
export function watchGlContextLoss(map: MaplibreMap, opts: GlContextRecoveryOptions): () => void {
  const container = map.getCanvasContainer();
  let order: string[] | null = null;
  /** The order was taken (and beforeLoss run) for the loss under way. */
  let snapshotTaken = false;
  /** A worker restart: beforeLoss runs on the rebuilt style. */
  let beforeLossOwed = false;
  let lost = false;

  const takeOrder = (): string[] | null => {
    try {
      return map.getLayersOrder();
    } catch {
      return null; // no style to snapshot: nothing custom is on it either
    }
  };

  // A context loss: the canvas event, before MapLibre serializes and destroys the style.
  const onCanvasLost = (): void => {
    // A second loss before the rebuilt style loaded (#576 RV18-MAP-2): the style
    // in place is still empty (its layer order is `[]`) and refuses writes, so
    // keep the order and the put-right from the last loaded style for whatever
    // brings a style back.
    if (lost) return;
    order = takeOrder();
    snapshotTaken = true;
    opts.beforeLoss?.();
  };

  // MapLibre's map event, on both paths. A worker restart had no canvas event and
  // its old style is still in place: take the order from it now.
  const onLoss = (): void => {
    if (!snapshotTaken) {
      order = takeOrder();
      snapshotTaken = true;
      beforeLossOwed = true;
    }
    lost = true;
  };

  const reattach = (): void => {
    if (!lost) return;
    lost = false;
    snapshotTaken = false;
    if (beforeLossOwed) {
      beforeLossOwed = false;
      opts.beforeLoss?.();
    }
    const layers = opts.layers();
    const owners = new Map(layers.map((l) => [l.layerId, l]));
    const snapshot = order;
    order = null;
    if (snapshot) reattachInOrder(snapshot, owners, (id) => Boolean(map.getLayer(id)));
    // A layer that was not on the map at the snapshot (held back while the style
    // was being rebuilt), or every layer without a snapshot: where a fresh add
    // puts it, bottom to top.
    for (const owner of layers) {
      if (!snapshot?.includes(owner.layerId)) owner.reattach(undefined);
    }
    opts.afterRestore();
    map.triggerRepaint();
  };

  container.addEventListener('webglcontextlost', onCanvasLost, true);
  const unsubscribe = onStyleRestored(map, reattach, onLoss);

  return () => {
    container.removeEventListener('webglcontextlost', onCanvasLost, true);
    unsubscribe();
  };
}
