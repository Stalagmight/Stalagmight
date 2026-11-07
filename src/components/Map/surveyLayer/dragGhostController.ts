// Drag-ghost controller (#mem drag latency): owns the moving-geometry preview of
// a move drag and drives it from a live screen-pixel offset.
//
// Two rendering paths, chosen per drag at begin:
//
//   CUSTOM (default while the bulk geometry is custom-rendered): a
//   {@link SurveyGhostLayer} WebGL layer whose vertices are the captured ORIGINAL
//   positions and whose live drag delta is ONE screen-px uniform, applied in the
//   vertex shader (surveyGhostLayer.ts / ghostShaders.ts). A pointermove is a
//   single uniform write + triggerRepaint - zero worker, zero rebuild, so there
//   is no async re-bake to overshoot. A mid-drag zoom stays correct because the
//   anchored vertices re-project through the frame matrix while the screen-px
//   offset remains the cursor residual MapCanvas recomputes each frame.
//
//   STOCK (fallback): the historical GeoJSON `survey-drag-preview` source +
//   {@link GhostFollow} single-flight bake / `line-translate` machine
//   (dragPreviewLayers.ts / dragGhostFollow.ts). Used when the bulk geometry is
//   NOT custom-rendered, when the custom ghost layer fails to initialize, or when
//   it self-disables on a GL error mid-drag (in which case the stock ghost is
//   spun up immediately at the live offset so the user is never left ghostless).
//
// SurveyLayer keeps the capture (the DragPreviewSet), the drag-exclusion of the
// originals, the interact-overlay follow, and the commit/flash sequencing; this
// controller only drives the ghost geometry.

import type * as maplibregl from 'maplibre-gl';
import { canWriteStyle, hasLiveStyle } from '../mapUtils';
import { GhostFollow } from './dragGhostFollow';
import {
  DRAG_PREVIEW_SOURCE,
  createDragPreviewLayers,
  removeDragPreviewLayers,
  setDragPreviewTranslate,
  type DragPreviewLayerCtx,
} from './dragPreviewLayers';
import {
  buildDragPreviewFeatureCollection,
  type DragPreviewSet,
  type Projector,
} from './dragPreviewSource';
import { SurveyGhostLayer, type GhostRenderData } from './customRender/surveyGhostLayer';
import {
  addCustomLayer,
  isStyleRestoring,
  reAddCustomLayer,
  removeCustomLayer,
} from './customRender/styleRestore';

/** Which ghost path is live for the current drag. */
export type GhostMode = 'idle' | 'custom' | 'stock';

/** Fallback timer if the stock bake's `sourcedata`-loaded confirmation is missed,
 *  so the single-flight queue cannot stall (mirrors the old SurveyLayer value). */
const GHOST_BAKE_TIMEOUT_MS = 250;

/** Everything the controller needs from SurveyLayer without importing it. */
export interface DragGhostOptions {
  /** Build the custom ghost render data (buffers + device-px sizes) for a set. */
  ghostData: (set: DragPreviewSet) => GhostRenderData;
  /** Stock-layer spec context (colors / widths / highlight). */
  layerCtx: () => DragPreviewLayerCtx;
  /** Map-backed projector for the stock per-frame re-bake. */
  projector: () => Projector;
  /** devicePixelRatio (css drag delta -> device px for the shader uniform). */
  dpr: () => number;
  /** Defer a style mutation off MapLibre's render stack (for self-disable). */
  scheduleRestore: (fn: () => void) => void;
}

export class DragGhostController {
  private mode: GhostMode = 'idle';
  private set: DragPreviewSet | null = null;
  /** Live drag delta in CSS px (last move); replayed on a mid-drag fallback. */
  private offset = { dx: 0, dy: 0 };

  // Custom path.
  private layer: SurveyGhostLayer | null = null;
  /** Guards the self-disable handler against re-entrancy while a restore pends. */
  private selfDisableScheduled = false;
  /** Bumped on every `end()` (and thus every `begin()`). A deferred self-disable
   *  restore pins the generation it was scheduled in and no-ops if a newer drag
   *  has since started, so drag N's stale callback can't tear down drag N+1's
   *  healthy custom ghost. */
  private dragGeneration = 0;

  // Stock path.
  private ghostFollow: GhostFollow | null = null;
  private landListener: ((e: maplibregl.MapSourceDataEvent) => void) | null = null;
  private bakeTimeout: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly map: maplibregl.Map,
    private readonly opts: DragGhostOptions,
    readonly layerId = 'survey-drag-ghost',
  ) {}

  /** True while a custom ghost layer is the live path (test / host read-back). */
  isCustom(): boolean {
    return this.mode === 'custom';
  }

  /** Begin a drag: build + show the ghost, returning which path went live so the
   *  host can time the drag-exclusion (custom draws next frame -> hide now; stock
   *  is async -> hide on the preview's first load).
   *
   *  The custom WebGL ghost is the DEFAULT for every drag regardless of the bulk
   *  geometry render mode - it is architecturally independent (its vertices are
   *  the captured moving copy, not the survey layer), so it fixes the overshoot on
   *  the default profile (custom labels + STOCK geometry) too. The stock GeoJSON
   *  ghost is the fail-safe only: the custom layer cannot initialize here, or it
   *  self-disables mid-drag on a GL error. */
  begin(set: DragPreviewSet): GhostMode {
    this.end();
    this.set = set;
    this.offset = { dx: 0, dy: 0 };
    // A pick-up while the WebGL context is lost (#576 NF-W4MAP-1): there is no
    // style, so neither path can add its layers - the stock one's getSource reads
    // the missing style and throws, and MapLibre's addLayer would lazily build an
    // EMPTY style to hold the custom one. Nothing could be drawn until the restore
    // anyway, so the carry stays idle (as the drag preview's begin paths skip their
    // style writes) and the next pick-up builds its ghost.
    if (!hasLiveStyle(this.map)) return 'idle';
    if (this.startCustom(set)) {
      this.mode = 'custom';
      return 'custom';
    }
    // Set the mode BEFORE startStock so the GhostFollow.start()'s initial bake@0
    // passes the bakeStock mode guard.
    this.mode = 'stock';
    this.startStock();
    return 'stock';
  }

  /** A pointermove (or zoom-synthesized re-apply) at the new cursor offset. */
  move(dx: number, dy: number): void {
    this.offset = { dx, dy };
    if (this.mode === 'custom') {
      const d = this.opts.dpr();
      this.layer?.setOffset(dx * d, dy * d);
      this.map.triggerRepaint();
    } else if (this.mode === 'stock') {
      this.ghostFollow?.move(dx, dy);
    }
  }

  /** Tear down both paths and remove the ghost (on drag end / cancel / destroy). */
  end(): void {
    // Invalidate any pending self-disable restore and clear the coalescing guard
    // so a stale drag's deferred callback can't fire against the next drag.
    this.dragGeneration++;
    this.selfDisableScheduled = false;
    this.disarmStock();
    // A carry cancelled while the WebGL context is lost: the style, and every
    // preview layer with it, is already gone (#576 NF-MAP-5).
    if (hasLiveStyle(this.map)) {
      removeDragPreviewLayers(this.map);
      removeCustomLayer(this.map, this.layer);
    }
    this.layer = null;
    this.set = null;
    this.mode = 'idle';
  }

  /**
   * Map-free teardown for the lightweight unmount (SurveyLayer.release): drop the
   * stock bake timer and land listener, and invalidate a pending self-disable
   * restore, WITHOUT removing layers or resetting the translate - the map is about
   * to be removed with everything on it. Otherwise the timer fires after the map
   * is gone and pins the whole SurveyLayer until it does. Idempotent; a later
   * {@link end} still removes whatever is left on a live map.
   */
  dispose(): void {
    this.dragGeneration++;
    this.selfDisableScheduled = false;
    this.dropStockListeners();
    this.ghostFollow = null;
    this.layer = null;
    this.set = null;
    this.mode = 'idle';
  }

  /**
   * Re-add a live custom ghost after MapLibre rebuilt the style without it (a
   * WebGL context restore or a worker restart; see glContextRecovery.ts).
   * reAddCustomLayer first frees what a worker restart left behind; without a
   * `beforeId` the ghost goes on top, as on start. The stock ghost needs nothing:
   * its source and layers are in the restored style. A re-add that cannot draw
   * falls back to the stock ghost, as a mid-drag self-disable does.
   */
  reattach(beforeId?: string): void {
    const layer = this.layer;
    if (this.mode !== 'custom' || !layer) return;
    if (!reAddCustomLayer(this.map, layer, beforeId)) {
      this.handleSelfDisable(layer.getDisabledReason() ?? 'unknown');
      return;
    }
    this.map.triggerRepaint();
  }

  /** Create + add the custom ghost layer. Returns false (so begin falls back to
   *  stock) if WebGL2 is unavailable / shaders fail / addLayer throws. */
  private startCustom(set: DragPreviewSet): boolean {
    try {
      const data = this.opts.ghostData(set);
      const layer = new SurveyGhostLayer(this.layerId, data, (reason) => this.handleSelfDisable(reason));
      // No beforeId: the ghost draws on top (above geometry + labels), matching
      // the stock drag-preview layers' stacking.
      addCustomLayer(this.map, layer);
      if (layer.isDisabled()) {
        removeCustomLayer(this.map, layer);
        console.warn(
          `[dragGhost] custom ghost failed to initialize (${layer.getDisabledReason() ?? 'unknown'}); ` +
            'using stock drag preview',
        );
        return false;
      }
      this.layer = layer;
      this.map.triggerRepaint();
      return true;
    } catch (e) {
      console.warn('[dragGhost] custom ghost threw on add; using stock drag preview', e);
      // The cleanup reads the style too; a style gone by now has no layer to drop.
      if (hasLiveStyle(this.map) && this.map.getLayer?.(this.layerId)) this.map.removeLayer(this.layerId);
      return false;
    }
  }

  /**
   * The custom ghost hit a GL error mid-drag and disabled itself (fired INSIDE
   * MapLibre's render pass, so a style mutation now is unsafe). Defer: drop the
   * dead custom layer and spin up the stock GeoJSON ghost at the LIVE offset so
   * the preview never vanishes. Idempotent + re-entrancy-safe.
   */
  private handleSelfDisable(reason: string): void {
    // A lost context fails every GL call; the restore's re-add rebuilds the layer.
    if (this.mode !== 'custom' || this.selfDisableScheduled || isStyleRestoring(this.map)) return;
    this.selfDisableScheduled = true;
    const gen = this.dragGeneration; // pin this drag; a later begin/end invalidates it
    console.warn(`[dragGhost] custom ghost self-disabled (${reason}); falling back to stock drag preview`);
    this.opts.scheduleRestore(() => {
      if (gen !== this.dragGeneration) return; // superseded by a newer drag / teardown
      this.selfDisableScheduled = false;
      if (this.mode !== 'custom' || !this.set) return;
      // The WebGL context was lost before this deferred fallback ran: there is no
      // style to build the stock ghost in, and addSource / addLayer read the
      // missing style and throw (#576 RV4-FE-MF-6, the NF-MAP-3 race); a rebuilt
      // style that has not loaded yet refuses them (RV18-MAP-1). Leave the dead
      // custom ghost in place: the restore's reattach() re-adds it, finds it
      // disabled and runs this fallback again on a live style.
      if (!canWriteStyle(this.map)) return;
      removeCustomLayer(this.map, this.layer);
      this.layer = null;
      this.mode = 'stock';
      this.startStock();
      // Replay the live offset so the stock ghost lands where the cursor is.
      this.ghostFollow?.move(this.offset.dx, this.offset.dy);
    });
  }

  private startStock(): void {
    createDragPreviewLayers(this.map, this.opts.layerCtx());
    this.armLand();
    this.ghostFollow = new GhostFollow({
      bake: (pos) => this.bakeStock(pos.dx, pos.dy),
      // Not while the WebGL context is lost: the preview layers went with the
      // style, and getLayer reads it (#576 RV4-FE-SL2).
      translate: (t) => {
        if (hasLiveStyle(this.map)) setDragPreviewTranslate(this.map, [t.dx, t.dy]);
      },
    });
    this.ghostFollow.start();
  }

  /** Re-project the captured set at `[dx, dy]` into ONE setData (the stock bake);
   *  arm the fallback land timer. Single-flight is enforced by {@link GhostFollow}. */
  private bakeStock(dx: number, dy: number): void {
    if (this.mode === 'idle' || !this.set) return;
    // While the WebGL context is lost there is no source to bake into (getSource
    // reads the missing style, #576 RV4-FE-SL2). The fallback land timer below
    // still runs, so the single-flight queue keeps moving until the restore.
    if (hasLiveStyle(this.map)) {
      const fc = buildDragPreviewFeatureCollection(this.set, this.opts.projector(), dx, dy);
      const src = this.map.getSource(DRAG_PREVIEW_SOURCE);
      if (src && 'setData' in src) (src as maplibregl.GeoJSONSource).setData(fc);
    }
    if (this.bakeTimeout !== null) clearTimeout(this.bakeTimeout);
    this.bakeTimeout = setTimeout(() => this.onBakeLanded(), GHOST_BAKE_TIMEOUT_MS);
  }

  private onBakeLanded(): void {
    if (this.bakeTimeout !== null) {
      clearTimeout(this.bakeTimeout);
      this.bakeTimeout = null;
    }
    this.ghostFollow?.land();
  }

  /** Arm the persistent `sourcedata` land confirmation for the stock bake. With
   *  single-flight baking, `isSourceLoaded` while a bake is in flight identifies
   *  THAT bake as landed. */
  private armLand(): void {
    if (this.landListener) return;
    const listener = (e: maplibregl.MapSourceDataEvent): void => {
      if (e.sourceId !== DRAG_PREVIEW_SOURCE) return;
      if (this.mode === 'idle') return;
      if (!this.ghostFollow?.hasBakeInFlight()) return;
      if (!this.map.isSourceLoaded(DRAG_PREVIEW_SOURCE)) return;
      this.onBakeLanded();
    };
    this.landListener = listener;
    this.map.on('sourcedata', listener);
  }

  private disarmStock(): void {
    this.dropStockListeners();
    // A stock carry cancelled while the WebGL context is lost: the preview layers
    // went with the style, so there is no translate to reset (#576 RV4-FE-SL2).
    if (this.ghostFollow && hasLiveStyle(this.map)) setDragPreviewTranslate(this.map, [0, 0]);
    this.ghostFollow = null;
  }

  /** The stock path's JS-only resources: the land listener and the bake timer. */
  private dropStockListeners(): void {
    if (this.landListener) {
      this.map.off('sourcedata', this.landListener);
      this.landListener = null;
    }
    if (this.bakeTimeout !== null) {
      clearTimeout(this.bakeTimeout);
      this.bakeTimeout = null;
    }
  }
}
