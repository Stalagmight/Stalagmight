/**
 * Controller wiring the custom WebGL render into GlobalSurveyLayer
 * (#customRender). Sibling of `Map/surveyLayer/customRender/controller.ts`,
 * keeping GlobalSurveyLayer to a few integration lines: it constructs one of
 * these, calls `sync(systems)` after each geometry flush, routes its
 * hover/select/search/reference/visibility/theme setters here, and asks `pick()`
 * to resolve interaction hits (replacing queryRenderedFeatures).
 *
 * The custom layer is the SOLE global-survey renderer (no gate, no stock layers):
 * `sync()` always adds it at the top of the global map (above the satellite
 * basemap; the system markers are DOM overlays, always above the canvas) and
 * drives everything from typed-array buffers + the geometry hit index. There is
 * NO stock fallback: an unrecoverable GL failure (no WebGL2, program link
 * failure, or a render-time `gl.getError` self-disable) removes the dead layer
 * and calls `onFatal(reason)` (error state). The stock global-survey layers +
 * their vector-tile publish were deleted in the stock-deletion stage, so there
 * is nothing to hide, restore, or republish.
 */
import type { Map as MaplibreMap } from 'maplibre-gl';
import type { ProjectMetadata } from '../../../types/project';
import type { SystemGeometry } from '../systemColor';
import { packCssColor } from '../../Map/surveyLayer/customRender/colorPack';
import {
  addCustomLayer,
  isStyleRestoring,
  reAddCustomLayer,
  removeCustomLayer,
} from '../../Map/surveyLayer/customRender/styleRestore';
import {
  buildGlobalBuffers,
  recomputeDimFlags,
  recomputeSelectedFlags,
  type GlobalBuildResult,
} from './globalTessellate';
import {
  GlobalCustomLayer,
  type GlobalCustomRenderData,
  type Rgba01,
} from './globalCustomLayer';
import { GlobalHitIndex, type GlobalHit, type ProjectFn } from './globalHitIndex';

/** Base visual weights (CSS px) for the custom global survey geometry. */
const BASE_SHOT_WIDTH = 2;
const BASE_STATION_RADIUS = 4;
const BASE_STATION_STROKE = 1;

/** Screen-space hit reaches (CSS px): station disc + shot line half-width. */
const STATION_REACH = 10;
const SHOT_REACH = 5;

/** Defer a fail-safe restore off MapLibre's render stack (rAF preferred; test
 *  fallbacks for non-DOM envs). Injectable for deterministic tests. */
function defaultScheduleRestore(fn: () => void): void {
  if (typeof requestAnimationFrame === 'function') requestAnimationFrame(() => fn());
  else if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

export interface GlobalCustomControllerOptions {
  /** Override how the fail-safe restore is deferred. Defaults to rAF. */
  scheduleRestore?: (fn: () => void) => void;
  /**
   * Notified whenever the custom layer's active (drawing) state flips: `true`
   * once added AND drawing, `false` on teardown / fail-safe self-disable.
   * Retained as a generic lifecycle signal (mirrors the single-system
   * controller); the stock vector-tile publish it once gated is gone, so
   * GlobalSurveyLayer no longer subscribes.
   */
  onActiveChange?: (active: boolean) => void;
  /** Called ONCE on an UNRECOVERABLE GL failure (no WebGL2, program link
   *  failure, or a render-time `gl.getError` self-disable). There is no stock
   *  fallback: the controller removes its dead layer and invokes this so the app
   *  can surface an error state. Mirrors the single-system controllers. */
  onFatal?: (reason: string) => void;
}

function packStroke(css: string): Rgba01 {
  const [r, g, b, a] = packCssColor(css);
  return [r / 255, g / 255, b / 255, a / 255];
}

export class GlobalCustomController {
  private readonly map: MaplibreMap;
  private readonly layerId: string;
  private readonly scheduleRestore: (fn: () => void) => void;
  private readonly onActiveChange: ((active: boolean) => void) | undefined;
  private readonly onFatal: ((reason: string) => void) | undefined;

  private layer: GlobalCustomLayer | null = null;
  private active = false;
  private selfDisableScheduled = false;
  /** Latched once an unrecoverable GL failure removes the layer: a fatal
   *  controller is dead and every later `sync()` bails (no stock fallback). */
  private fatal = false;
  /** Guards {@link emitFatal} so {@link onFatal} fires at most once. */
  private fatalEmitted = false;

  // Current systems + derived state, held so a (re)enable rebuilds correctly.
  private systems: SystemGeometry[] = [];
  private metadataById = new Map<string, ProjectMetadata>();
  private hitIndex: GlobalHitIndex | null = null;
  private build: GlobalBuildResult | null = null;
  private data: GlobalCustomRenderData | null = null;

  private searchTerm: string | null = null;
  private selectedIds: ReadonlySet<string> = new Set();
  private referenceVisible = true;
  private visible = true;
  private strokeColor: Rgba01 = [1, 1, 1, 1];

  constructor(map: MaplibreMap, layerId = 'global-survey-custom', opts: GlobalCustomControllerOptions = {}) {
    this.map = map;
    this.layerId = layerId;
    this.scheduleRestore = opts.scheduleRestore ?? defaultScheduleRestore;
    this.onActiveChange = opts.onActiveChange;
    this.onFatal = opts.onFatal;
  }

  /** Whether the custom layer is drawing (stock layers hidden). */
  isActive(): boolean {
    return this.active;
  }

  /** Metadata for a system id (tooltip lookup for the resolved hit). */
  getSystemMetadata(systemId: string): ProjectMetadata | undefined {
    return this.metadataById.get(systemId);
  }

  /** Build/refresh the custom layer (the sole global-survey renderer - no gate). */
  sync(systems: SystemGeometry[]): void {
    // A fatal (dead GL) controller never rebuilds; the survey stays blank.
    if (this.fatal) return;
    this.systems = systems;
    this.metadataById = new Map(systems.map((s) => [s.metadata.file_path, s.metadata]));
    this.hitIndex = new GlobalHitIndex(systems);

    const data = this.buildData();
    if (!this.active) this.enable(data);
    else this.layer?.update(data);
  }

  /** Rebuild from the held systems - e.g. after a device-pixel-ratio change, which
   *  invalidates the baked device-pixel sizes. No-op unless already drawing. */
  resync(): void {
    if (this.active) this.sync(this.systems);
  }

  private buildData(): GlobalCustomRenderData {
    const build = buildGlobalBuffers(this.systems, {
      searchTerm: this.searchTerm,
      selectedIds: this.selectedIds,
    });
    this.build = build;
    const dpr = (typeof window !== 'undefined' && window.devicePixelRatio) || 1;
    const data: GlobalCustomRenderData = {
      anchorX: build.anchorX,
      anchorY: build.anchorY,
      line: build.line,
      circle: build.circle,
      lineHalfWidthPx: (BASE_SHOT_WIDTH / 2) * dpr,
      circleRadiusPx: BASE_STATION_RADIUS * dpr,
      circleStrokePx: BASE_STATION_STROKE * dpr,
      aaPx: dpr,
      strokeColor: this.strokeColor,
    };
    this.data = data;
    return data;
  }

  private enable(data: GlobalCustomRenderData): void {
    if (!this.map.getStyle) return;
    const layer = new GlobalCustomLayer(this.layerId, data, (reason) => this.handleSelfDisable(reason));
    layer.setVisible(this.visible);
    layer.setReferenceShotsVisible(this.referenceVisible);
    layer.setHoverSystem(-1);
    // Add at the top of the global map: the only other GL layer is the satellite
    // basemap (below), and the system markers are DOM overlays that always sit
    // above the canvas, so no beforeId anchor is needed.
    addCustomLayer(this.map, layer);
    if (layer.isDisabled()) {
      // FATAL: the custom layer cannot draw. There is NO stock fallback - remove
      // the dead layer and route to the error state instead of restoring stock.
      const reason = layer.getDisabledReason() ?? 'unknown';
      console.warn(
        `[globalCustomRender] custom layer failed to initialize (${reason}); ` +
          'survey rendering unavailable',
      );
      removeCustomLayer(this.map, layer);
      this.layer = null;
      this.fatal = true;
      this.setActive(false); // active was never true here, so this does not fire.
      this.emitFatal(reason);
      return;
    }
    this.layer = layer;
    this.setActive(true);
    console.info('[globalCustomRender] enabled', {
      systems: this.systems.length,
      shotSegments: data.line.segmentCount,
      stations: data.circle.stationCount,
    });
    this.map.triggerRepaint();
  }

  /** Re-add the layer after MapLibre rebuilt the style without it (context
   *  restore / worker restart, see styleRestore.ts). */
  reattach(): void {
    const layer = this.layer;
    if (!this.active || !layer) return;
    if (!reAddCustomLayer(this.map, layer)) {
      this.tearDownFatal(layer.getDisabledReason() ?? 'unknown');
      return;
    }
    this.map.triggerRepaint();
  }

  /** Remove the custom layer (the sole global-survey renderer - no stock to
   *  restore or republish). Safe when already inactive. Non-fatal teardown only
   *  (destroy); the fatal path uses {@link tearDownFatal}. */
  private teardown(): void {
    this.setActive(false);
    removeCustomLayer(this.map, this.layer);
    this.layer = null;
    this.map.triggerRepaint();
  }

  private handleSelfDisable(reason: string): void {
    // A lost context fails every GL call; the restore's re-add rebuilds the layer.
    if (!this.active || this.selfDisableScheduled || isStyleRestoring(this.map)) return;
    this.selfDisableScheduled = true;
    console.warn(
      `[globalCustomRender] custom layer self-disabled (${reason}); survey rendering unavailable`,
    );
    this.scheduleRestore(() => {
      this.selfDisableScheduled = false;
      if (this.active) this.tearDownFatal(reason);
    });
  }

  /**
   * FATAL teardown: remove the dead layer, latch {@link fatal} so a later sync()
   * bails, and surface the error via {@link onFatal}. Sets `active` directly
   * (skipping {@link onActiveChange}) so the fatal path stays a pure error-state
   * signal; there are no stock layers left to restore or republish.
   */
  private tearDownFatal(reason: string): void {
    this.fatal = true;
    removeCustomLayer(this.map, this.layer);
    this.layer = null;
    this.active = false; // set directly: onActiveChange must NOT fire (no republish).
    this.map.triggerRepaint();
    this.emitFatal(reason);
  }

  /** Fire {@link onFatal} at most once. */
  private emitFatal(reason: string): void {
    if (this.fatalEmitted) return;
    this.fatalEmitted = true;
    this.onFatal?.(reason);
  }

  private setActive(v: boolean): void {
    if (this.active === v) return;
    this.active = v;
    this.onActiveChange?.(v);
  }

  /** Hovered system (or null). Uniform-only refresh, no buffer re-upload. */
  setHover(systemId: string | null): void {
    const idx = systemId !== null ? this.build?.systemIndex.get(systemId) ?? -1 : -1;
    this.layer?.setHoverSystem(idx);
    if (this.active) this.map.triggerRepaint();
  }

  /** Selected systems. Recomputes the per-vertex selected flags + re-uploads. */
  setSelected(selectedIds: ReadonlySet<string>): void {
    this.selectedIds = new Set(selectedIds);
    if (!this.active || !this.layer || !this.build || !this.data) return;
    const { systemIndex } = this.build;
    const indexToId = new Map<number, string>();
    for (const [id, i] of systemIndex) indexToId.set(i, id);
    const selForIndex = (i: number): boolean => {
      const id = indexToId.get(i);
      return id !== undefined && this.selectedIds.has(id);
    };
    recomputeSelectedFlags(this.data.line, this.data.circle, selForIndex);
    this.layer.updateSelected(this.data.line.selected, this.data.circle.selected);
    this.map.triggerRepaint();
  }

  /** Search term (dims non-matching systems). Recomputes dim flags + re-uploads. */
  setSearch(term: string | null): void {
    this.searchTerm = term ? term.toLowerCase() : null;
    if (!this.active || !this.layer || !this.build || !this.data) return;
    const { systemIndex } = this.build;
    const indexToName = new Map<number, string>();
    for (const s of this.systems) {
      const i = systemIndex.get(s.metadata.file_path);
      if (i !== undefined) indexToName.set(i, s.metadata.name);
    }
    const dimForIndex = (i: number): boolean => {
      if (!this.searchTerm) return false;
      const name = indexToName.get(i);
      return name !== undefined && !name.toLowerCase().includes(this.searchTerm);
    };
    recomputeDimFlags(this.data.line, this.data.circle, dimForIndex);
    this.layer.updateDim(this.data.line.dim, this.data.circle.dim);
    this.map.triggerRepaint();
  }

  /** Hide/show reference shots + all-reference stations. */
  setReferenceShotsVisible(visible: boolean): void {
    this.referenceVisible = visible;
    this.layer?.setReferenceShotsVisible(visible);
    if (this.active) this.map.triggerRepaint();
  }

  /** Show/hide the whole custom survey (markers-only view). */
  setVisible(visible: boolean): void {
    this.visible = visible;
    this.layer?.setVisible(visible);
    if (this.active) this.map.triggerRepaint();
  }

  /** Apply a new station stroke color (theme change). Uniform-only. */
  setStrokeColor(css: string): void {
    this.strokeColor = packStroke(css);
    if (this.data) this.data.strokeColor = this.strokeColor;
    if (this.active) this.map.triggerRepaint();
  }

  /**
   * The system under a screen point (station-first then shot), or null. Returns
   * null unless the custom layer is active + visible, and honors the reference
   * shots visibility, so hidden features stay non-interactive.
   */
  pick(project: ProjectFn, x: number, y: number, cameraKey: string | null = null): GlobalHit | null {
    if (!this.active || !this.visible || !this.hitIndex) return null;
    return this.hitIndex.pick(
      project,
      x,
      y,
      {
        includeReference: this.referenceVisible,
        stationReach: STATION_REACH,
        shotReach: SHOT_REACH,
      },
      cameraKey,
    );
  }

  destroy(): void {
    if (this.active) this.teardown();
    this.systems = [];
    this.metadataById = new Map();
    this.hitIndex = null;
    this.build = null;
    this.data = null;
  }
}
