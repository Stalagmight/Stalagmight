// Measure-path controller (#26, #customRender stock deletion). Owns the dedicated
// {@link SurveyMeasurePathLayer} that draws the ruler's measured along-cave-line
// tint (measured shots as round-cap lines, measured stations as filled discs, one
// tint via a uniform) directly from typed-array buffers - no stock
// `survey-measure-path` layer, no `-shots`/`-stations` vector-tile source, no
// `measured` feature-state. This is the SOLE measure-path renderer; the stock
// path is gone.
//
// SurveyLayer builds the render data (resolving the measured id sets to coords via
// its geometry index) and calls `setPath(...)` / `clear()`; this controller only
// owns the layer lifecycle: add at the right z-order, re-upload on change, remove
// on clear, and - since there is no stock tint behind it - surface a GL failure
// via `onFatal` while dropping the dead layer, rather than vanishing silently.

import type * as maplibregl from 'maplibre-gl';
import { SurveyMeasurePathLayer, type MeasurePathRenderData } from './surveyMeasurePathLayer';
import {
  addCustomLayer,
  isStyleRestoring,
  reAddCustomLayer,
  removeCustomLayer,
  type RestorableCustomLayer,
} from './styleRestore';

/** The surface the controller uses from its GL layer (real or a test fake). */
export interface MeasurePathLayerLike {
  readonly id: string;
  isDisabled(): boolean;
  getDisabledReason(): string | null;
  setData(data: MeasurePathRenderData): void;
}

/** Injectable seams (defaults are the real layer + rAF). Tests override both. */
export interface MeasurePathDeps {
  /** Build the GL layer. Defaults to a real {@link SurveyMeasurePathLayer}. */
  makeLayer?: (
    id: string,
    data: MeasurePathRenderData | null,
    onSelfDisable: (reason: string) => void,
  ) => MeasurePathLayerLike;
  /** Defer a style mutation off MapLibre's render stack (self-disable removal). */
  scheduleRestore?: (fn: () => void) => void;
  /** Unrecoverable-GL-failure callback. There is no stock fallback anymore, so a
   *  measure-path layer that fails to initialize or self-disables mid-render must
   *  surface the failure (a toast) rather than vanish silently. SurveyLayer routes
   *  this to its shared render-fatal handler. */
  onFatal?: (reason: string) => void;
}

function defaultScheduleRestore(fn: () => void): void {
  if (typeof requestAnimationFrame === 'function') requestAnimationFrame(() => fn());
  else if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

export class MeasurePathController {
  private layer: MeasurePathLayerLike | null = null;
  private readonly makeLayer: NonNullable<MeasurePathDeps['makeLayer']>;
  private readonly scheduleRestore: (fn: () => void) => void;
  /** Bumped on every clear/destroy so a deferred self-disable removal that a
   *  newer path superseded is a no-op (mirrors the ghost controller's guard). */
  private generation = 0;
  private selfDisableScheduled = false;
  private readonly onFatal: ((reason: string) => void) | undefined;
  /** Guards {@link onFatal} so it fires at most once per controller. */
  private fatalEmitted = false;
  /** Last render data handed to the layer, for {@link drawnCounts}. */
  private lastData: MeasurePathRenderData | null = null;
  /** z-order anchors of the last {@link setPath}, replayed by {@link reattach}. */
  private beforeIdCandidates: readonly string[] = [];

  constructor(
    private readonly map: maplibregl.Map,
    deps: MeasurePathDeps = {},
    private readonly layerId = 'survey-measure-path-custom',
  ) {
    this.makeLayer =
      deps.makeLayer ??
      ((id, data, onSelfDisable) => new SurveyMeasurePathLayer(id, data, onSelfDisable));
    this.scheduleRestore = deps.scheduleRestore ?? defaultScheduleRestore;
    this.onFatal = deps.onFatal;
  }

  /** Fire {@link onFatal} at most once. */
  private emitFatal(reason: string): void {
    if (this.fatalEmitted) return;
    this.fatalEmitted = true;
    this.onFatal?.(`measure-path: ${reason}`);
  }

  /** True while the custom measure-path layer is added and drawing (test/host). */
  isActive(): boolean {
    return this.layer !== null;
  }

  /**
   * How much measured path is currently drawn: tinted shot segments and tinted
   * station discs. Zeroes when nothing is drawn (cleared or never set).
   *
   * The measure tint is a custom WebGL layer with no MapLibre feature-state, so
   * this is the observable an E2E test asserts on instead of reading a `measured`
   * feature-state off a (deleted) vector-tile source.
   */
  drawnCounts(): { shots: number; stations: number } {
    const data = this.lastData;
    if (!this.layer || !data) return { shots: 0, stations: 0 };
    return { shots: data.line.segmentCount, stations: data.circle.pointCount };
  }

  /**
   * Show / update the measured path. Pass render data with content to draw it;
   * pass null (or data whose line + circle are both empty) to clear.
   *
   * `beforeIdCandidates` are tried in order; the layer is inserted before the
   * first one that exists so it lands above the survey geometry and directly
   * beneath the hover/selection overlay - see SurveyLayer. If none exist the
   * layer draws on top.
   */
  setPath(data: MeasurePathRenderData | null, beforeIdCandidates: readonly string[] = []): void {
    const empty = !data || (data.line.segmentCount === 0 && data.circle.pointCount === 0);
    if (empty) {
      this.clear();
      return;
    }
    this.beforeIdCandidates = beforeIdCandidates;
    if (this.layer) {
      this.lastData = data;
      this.layer.setData(data);
      this.map.triggerRepaint();
      return;
    }
    this.add(data, beforeIdCandidates);
  }

  /** Re-add the layer after MapLibre rebuilt the style without it (context
   *  restore / worker restart, see styleRestore.ts). */
  reattach(): void {
    const layer = this.layer;
    if (!layer) return;
    const restorable = layer as unknown as RestorableCustomLayer;
    if (!reAddCustomLayer(this.map, restorable, this.resolveBeforeId(this.beforeIdCandidates))) {
      removeCustomLayer(this.map, layer);
      this.layer = null;
      this.lastData = null;
      this.emitFatal(layer.getDisabledReason() ?? 'unknown');
      return;
    }
    this.map.triggerRepaint();
  }

  /** Remove the custom measure-path layer (idempotent). */
  clear(): void {
    this.generation++;
    this.selfDisableScheduled = false;
    removeCustomLayer(this.map, this.layer);
    this.layer = null;
    this.lastData = null;
  }

  /** Tear down on SurveyLayer destroy. */
  destroy(): void {
    this.clear();
  }

  /** Create + add the layer at the resolved z-order. Drops the dead layer and
   *  surfaces the failure via {@link onFatal} if WebGL2 is unavailable / shaders
   *  fail / it throws - there is no stock tint to fall back to. */
  private add(data: MeasurePathRenderData, beforeIdCandidates: readonly string[]): void {
    try {
      const layer = this.makeLayer(this.layerId, data, (reason) => this.handleSelfDisable(reason));
      const beforeId = this.resolveBeforeId(beforeIdCandidates);
      addCustomLayer(this.map, layer as unknown as maplibregl.CustomLayerInterface, beforeId);
      if (layer.isDisabled()) {
        const reason = layer.getDisabledReason() ?? 'unknown';
        removeCustomLayer(this.map, layer);
        console.warn(`[measurePath] custom measure-path failed to initialize (${reason})`);
        this.emitFatal(reason);
        return;
      }
      this.layer = layer;
      this.lastData = data;
      this.map.triggerRepaint();
    } catch (e) {
      console.warn('[measurePath] custom measure-path threw on add', e);
      if (this.map.getLayer?.(this.layerId)) this.map.removeLayer(this.layerId);
      this.layer = null;
      this.lastData = null;
      this.emitFatal(e instanceof Error ? e.message : 'add-threw');
    }
  }

  private resolveBeforeId(candidates: readonly string[]): string | undefined {
    for (const id of candidates) {
      if (this.map.getLayer?.(id)) return id;
    }
    return undefined;
  }

  /**
   * The layer hit a GL error mid-render and disabled itself (fired INSIDE
   * MapLibre's render pass, so a style mutation now is unsafe). Defer dropping
   * the dead layer, then surface the failure via {@link onFatal} - there is no
   * stock tint behind it, so a measured path silently loses its highlight
   * otherwise. Idempotent + generation-guarded.
   */
  private handleSelfDisable(reason: string): void {
    // A lost context fails every GL call; the restore's re-add rebuilds the layer.
    if (!this.layer || this.selfDisableScheduled || isStyleRestoring(this.map)) return;
    this.selfDisableScheduled = true;
    const gen = this.generation;
    console.warn(`[measurePath] custom measure-path self-disabled (${reason})`);
    this.scheduleRestore(() => {
      if (gen !== this.generation) return; // superseded by a clear / new path
      this.selfDisableScheduled = false;
      removeCustomLayer(this.map, this.layer);
      this.layer = null;
      this.emitFatal(reason);
    });
  }
}
