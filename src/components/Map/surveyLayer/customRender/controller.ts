/**
 * Controller wiring the custom-render prototype into SurveyLayer (#customRender).
 *
 * Keeps SurveyLayer.tsx to a few integration lines: SurveyLayer constructs one
 * of these, calls `sync(...)` after each geometry flush, and `destroy()` on
 * teardown. Everything else - building buffers from SoA, resolving colors,
 * adding/removing the custom layer, and toggling the stock bulk layers'
 * visibility - lives here.
 *
 * The custom layer is the SOLE survey renderer: `sync()` always builds/enables
 * it (no gate), adding it before `survey-interact-shots` (above basemap/ghosts,
 * below the interact hover/selection overlay + labels). The stock survey renderer
 * has been deleted, so there is NO fallback: an unrecoverable GL failure (no
 * WebGL2, program link failure, or a render-time `gl.getError` self-disable)
 * removes the dead layer and calls `onFatal(reason)` so the app surfaces an error
 * state (a toast) rather than falling back to stock.
 *
 * Hit-testing does NOT depend on the stock layers: candidate-section discovery
 * runs against the SoA geometry index (SurveyLayer.hitCandidateSections ->
 * geometryIndex.sectionsIntersectingBox), so hiding the stock layers leaves
 * hover / select / station+segment move fully working. Labels, overlays, and
 * the vector-tile publish path are likewise untouched.
 */
import type { Map as MaplibreMap } from 'maplibre-gl';
import type { GpsOrigin } from '../../../../types/config';
import { type SurveyData, stationKindRank } from '../../../../types/survey-data';
import { metersToMapCoords } from '../../../../utils/coordinates';
import { debugLog } from '../../../../utils/debugLog';
import { FEATURE_ID_STRIDE } from '../../../../utils/ids';
import { gasBeyondReachFeatureIds, type GasReachFeatureState } from '../../../../utils/gasReach';
import type { DisplayColorContext } from '../displayColor';
import { lngLatToMercator } from '../mercatorCoords';
import { computeMercatorAnchor } from './mercator';
import {
  buildStationLookup,
  computeStationAllReference,
  projectStationsRel,
  buildLineBuffers,
  buildCircleBuffers,
  computeLineDim,
  computeCircleDim,
  recomputeLineSection,
  recomputeStation,
  recomputeSectionAllReference,
  writeCircleDrawOrder,
  recomputeLineColors,
  recomputeCircleColors,
  type StationRel,
  type CustomRenderExclusion,
  type LineSectionInputs,
  type CircleStationInputs,
} from './tessellate';
import { makeColorResolvers } from './colorResolvers';
import { planIncrementalUpdate, planColorOnlyUpdate, coalesceVertexRuns } from './incremental';
import { SurveyCustomLayer, type CustomRenderData } from './surveyCustomLayer';
import {
  highlightSizeMul,
  DEFAULT_SHOT_HOVER_WIDTH_MUL,
  DEFAULT_SHOT_SELECT_WIDTH_MUL,
  DEFAULT_STATION_HOVER_SIZE_MUL,
  DEFAULT_STATION_SELECT_SIZE_MUL,
} from './shaders';
import { CUSTOM_RENDER_SYNC_HOOK } from './gate';
import { addCustomLayer, isStyleRestoring, reAddCustomLayer, removeCustomLayer } from './styleRestore';

/**
 * Defer a fail-safe restore off MapLibre's render stack. A self-disable fires
 * from inside `render()`, so we must NOT mutate the style synchronously there.
 * rAF (preferred) runs it next frame; microtask / setTimeout are non-DOM
 * fallbacks (test/node). Injectable via controller options for deterministic tests.
 */
function defaultScheduleRestore(fn: () => void): void {
  if (typeof requestAnimationFrame === 'function') requestAnimationFrame(() => fn());
  else if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

export interface SurveyCustomControllerOptions {
  /** Override how the fail-safe restore is deferred off the render stack.
   *  Defaults to {@link defaultScheduleRestore}; tests inject an inline runner. */
  scheduleRestore?: (fn: () => void) => void;
  /**
   * Re-sync the layer from CURRENT geometry, backing the runtime
   * `__stalagmightSyncCustomRender` hook. When provided, the hook calls this
   * instead of replaying the controller's cached `lastParams` snapshot: after a
   * project switch the snapshot is a previous project's geometry, so replaying
   * it strands the custom layer on stale data (fewer shots than the store).
   * SurveyLayer supplies a resync that reads the store, so a manual re-sync
   * always converges to the loaded project. Absent (unit tests): fall back to
   * the cached snapshot.
   */
  resync?: () => void;
  /**
   * Notified whenever the custom layer's active (drawing) state flips. `true`
   * once the layer is added AND drawing (stock bulk layers hidden); `false` on
   * teardown, gate-off, or a fail-safe self-disable. SurveyLayer uses this to
   * stop publishing the stock bulk shot/station/gradient vector tiles while the
   * custom layer covers them, and to re-publish them from cached SoA when the
   * custom layer goes away (#customRender / #mem). Never fires for a failed
   * enable (the layer that could not draw leaves `active` false, so the stock
   * pipeline must stay published).
   */
  onActiveChange?: (active: boolean) => void;
  /**
   * Called ONCE when the custom layer hits an UNRECOVERABLE GL failure (no
   * WebGL2, program link failure, or a render-time `gl.getError` self-disable).
   * There is no stock fallback, so the controller removes its own dead layer and
   * invokes this so the app can surface a user-facing error state (the survey
   * area is now blank). `reason` is the layer's disabled reason (e.g.
   * `webgl2-unavailable`, `program-link-failed`, `gl-error-0x502`).
   */
  onFatal?: (reason: string) => void;
}

export interface CustomSyncParams {
  surveyData: SurveyData;
  gpsOrigin: GpsOrigin | null;
  colorCtx: DisplayColorContext;
  /** CSS-pixel style sizes (mirror the stock layers' visual weights). */
  shotWidthPx: number;
  stationRadiusPx: number;
  stationStrokePx: number;
  /** Configured highlight sizes (C2), absolute CSS px. When present they drive
   *  the shot line + station radius bump for a hovered / selected section via
   *  ratios over the base size; absent = the historical 2x/1.5x (shot) and
   *  1.5x/1.25x (station) defaults. Independent config keys `map.shot.hover_width`
   *  / `selection_width` + `map.station.hover_radius` / `selection_radius`. */
  shotHoverWidthPx?: number | undefined;
  shotSelectionWidthPx?: number | undefined;
  stationHoverRadiusPx?: number | undefined;
  stationSelectionRadiusPx?: number | undefined;
  /** Features to omit from the buffers, mirroring the stock bulk layers'
   *  move-drag setFilter exclusions. Absent / undefined = draw everything. */
  exclude?: CustomRenderExclusion | undefined;
}

export type { CustomRenderExclusion } from './tessellate';

/**
 * Current dimming set mirroring the stock move-tool paint state (#customRender).
 * `anchoredSectionIds` dims every station + shot of those sections (move-segment,
 * SurveyLayer.setAnchoredSections); the immovable `featureId` sets dim specific
 * stations / shots (move-station / move-shot, SurveyLayer.setImmovableFeatures).
 * `featureId = sectionId * FEATURE_ID_STRIDE + stationIndex | shotId`, matching
 * the stock layers' `['get','featureId']` matching. Held on the controller so a
 * layer re-create (re-enable / re-sync) re-applies it during the next build.
 *
 * `gasDim*FeatureIds` (#395) are the stations/shots beyond the active gas-reach
 * budget: dimmed at the lighter {@link GAS_DIM_LEVEL} (0.5) rather than the full
 * anchored/immovable dim (1.0) - full dim wins on overlap (see {@link dimPredicates}).
 */
export interface CustomDimState {
  anchoredSectionIds: ReadonlySet<number>;
  immovableStationFeatureIds: ReadonlySet<number>;
  immovableShotFeatureIds: ReadonlySet<number>;
  gasDimStationFeatureIds: ReadonlySet<number>;
  gasDimShotFeatureIds: ReadonlySet<number>;
  /** Plan-mode idle tint (#395 revision): true dims EVERY station/shot at the
   *  lighter {@link GAS_DIM_LEVEL} (0.5), mirroring "beyond gas range" before an
   *  origin is chosen - full anchored/immovable dim (1.0) still wins. */
  planIdleDim: boolean;
}

/** Shared empty-`Set<number>` sentinel, reused by both the dim state below
 *  (`EMPTY_DIM`, `buildCustomDimState`'s gas-off branches) and the highlight
 *  state fields further down (`segmentHoverState` et al.) - declared before its
 *  first use so callers don't rely on `const` hoisting behavior specific to
 *  function bodies. */
const EMPTY_FEATURE_SET: ReadonlySet<number> = new Set<number>();

const EMPTY_DIM: CustomDimState = {
  anchoredSectionIds: EMPTY_FEATURE_SET,
  immovableStationFeatureIds: EMPTY_FEATURE_SET,
  immovableShotFeatureIds: EMPTY_FEATURE_SET,
  gasDimStationFeatureIds: EMPTY_FEATURE_SET,
  gasDimShotFeatureIds: EMPTY_FEATURE_SET,
  planIdleDim: false,
};

/** The per-vertex `a_dim` / `v_dim` level for "beyond gas range" (#395): between
 *  none (0) and the full anchored/immovable dim (1.0). See `shaders.ts` for the
 *  shader-side threshold mapping. */
export const GAS_DIM_LEVEL = 0.5;

/** Assemble a {@link CustomDimState} from SurveyLayer's held sets + the current
 *  gas-reach mode/state (#395) - the single place the gas-dim sets are derived,
 *  shared by the geometry `setDimming` and label `setDimming` calls SurveyLayer
 *  pushes the SAME state to. */
export function buildCustomDimState(
  anchoredSectionIds: ReadonlySet<number>,
  immovableStationFeatureIds: ReadonlySet<number>,
  immovableShotFeatureIds: ReadonlySet<number>,
  gasReachMode: boolean,
  gasReachState: GasReachFeatureState | null,
  planIdleDim = false,
): CustomDimState {
  return {
    anchoredSectionIds, immovableStationFeatureIds, immovableShotFeatureIds,
    gasDimStationFeatureIds: gasReachMode ? gasBeyondReachFeatureIds(gasReachState?.stations) : EMPTY_FEATURE_SET,
    gasDimShotFeatureIds: gasReachMode ? gasBeyondReachFeatureIds(gasReachState?.shots) : EMPTY_FEATURE_SET,
    planIdleDim,
  };
}

/** No features hidden (drag inactive): the zero-cost default for the hide flag. */
const EMPTY_EXCLUSION: CustomRenderExclusion = {};

/** Emphasis levels stored in the per-vertex highlight flag, mirroring the stock
 *  feature-state bumps. Hover wins over selection when a section is in both
 *  (matches the stock paint case order: `hover` before `selected`). */
const HIGHLIGHT_HOVER = 2;
const HIGHLIGHT_SELECT = 1;
const HIGHLIGHT_NONE = 0;

export class SurveyCustomController {
  private readonly map: MaplibreMap;
  private readonly layerId: string;
  private layer: SurveyCustomLayer | null = null;
  private active = false;
  private lastParams: CustomSyncParams | null = null;
  private hookInstalled = false;
  private readonly scheduleRestore: (fn: () => void) => void;
  /** Fresh-geometry re-sync backing the runtime hook; see options.resync. */
  private readonly resync: (() => void) | undefined;
  /** Active-state transition callback; see options.onActiveChange. */
  private readonly onActiveChange: ((active: boolean) => void) | undefined;
  /** Unrecoverable-GL-failure callback; see options.onFatal. */
  private readonly onFatal: ((reason: string) => void) | undefined;
  /** Latched once an unrecoverable GL failure removes the layer. A fatal
   *  controller is dead: every later `sync()` bails, so the survey stays blank
   *  (error state) instead of flip-flopping on a re-enable that would fail
   *  again. There is no stock to fall back to. */
  private fatal = false;
  /** Guards {@link emitFatal} so {@link onFatal} fires at most once. */
  private fatalEmitted = false;
  /** Guards handleSelfDisable against re-entrancy while a restore is pending. */
  private selfDisableScheduled = false;
  /** Set once {@link dispose} / {@link destroy} runs. A late runtime-hook fire or
   *  deferred sync bails on it so a torn-down controller never rebuilds. */
  private disposed = false;
  /** Display toggles mirroring the stock layers' setShowShots /
   *  setShowStations / setShowReferenceShots. Held on the controller so they
   *  survive a layer re-create (enable re-applies them to the fresh layer). */
  private shotsVisible = true;
  private stationsVisible = true;
  private referenceShotsVisible = true;
  /** #27: jumps hide independently of references. */
  private jumpShotsVisible = true;
  /** Anchored / immovable dim set applied to every build; survives re-enable. */
  private dimState: CustomDimState = EMPTY_DIM;
  /** Features hidden for the active move drag (drag begin/end). Applied as the
   *  per-vertex hide flag over the FULL buffers instead of omitting features (a
   *  rebuild), and re-applied on every build so a mid-drag flush stays hidden. */
  private hideState: CustomRenderExclusion = EMPTY_EXCLUSION;
  /** The three section-emphasis sources, merged per section into a highlight
   *  LEVEL (hover 2 > selection 1 > none 0) applied as the per-vertex highlight
   *  flag and re-applied on every build so a mid-interaction flush stays
   *  emphasized. `segmentHoverState` is the Move-Segment connected set
   *  (setHighlight); `sectionHoverState` is single-section / panel-row hover
   *  (setSectionHover); `sectionSelectState` is the selected section
   *  (setSectionSelect). Hover (segment or section) both map to level 2, matching
   *  the stock `hover` feature-state; selection maps to level 1 (stock
   *  `selected`). Anchored sections are excluded from segment-hover upstream. */
  private segmentHoverState: ReadonlySet<number> = EMPTY_FEATURE_SET;
  private sectionHoverState: ReadonlySet<number> = EMPTY_FEATURE_SET;
  private sectionSelectState: ReadonlySet<number> = EMPTY_FEATURE_SET;
  /** Cached build products backing the cheap dim-only refresh (setDimming),
   *  the drag hide refresh (setDragHide), and the incremental edit path: the
   *  last CustomRenderData (its per-vertex arrays are rewritten in place) plus
   *  the station lookup / anchor-relative positions / all-reference flags /
   *  survey the recomputes need. */
  private lastData: CustomRenderData | null = null;
  private lastLookup: Map<number, Map<number, number>> | null = null;
  private lastSurveyData: SurveyData | null = null;
  private lastStationRel: StationRel | null = null;
  private lastAllReference: Float32Array | null = null;

  constructor(
    map: MaplibreMap,
    layerId = 'survey-custom-render',
    opts: SurveyCustomControllerOptions = {},
  ) {
    this.map = map;
    this.layerId = layerId;
    this.scheduleRestore = opts.scheduleRestore ?? defaultScheduleRestore;
    this.resync = opts.resync;
    this.onActiveChange = opts.onActiveChange;
    this.onFatal = opts.onFatal;
    this.installHook();
  }

  /** Whether the custom layer is currently drawing. */
  isActive(): boolean {
    return this.active;
  }

  /** Build/refresh the custom layer (the sole survey renderer - no gate). */
  sync(params: CustomSyncParams): void {
    // A disposed (unmounted) or fatal (dead GL) controller never rebuilds.
    if (this.disposed || this.fatal) return;
    const prevParams = this.lastParams;
    this.lastParams = params;
    // Fast paths (active layer + a prior snapshot to diff against): a geometry
    // edit that left the SoA layout unchanged re-emits just the changed sections;
    // a pure global recolor (overlay / theme / depth-mode toggle) with unchanged
    // geometry re-uploads just the two color buffers. Either beats a full rebuild.
    if (this.active && this.layer && prevParams) {
      if (this.tryIncremental(prevParams, params)) return;
      if (this.tryColorOnly(prevParams, params)) return;
    }
    const data = this.build(params);
    if (!this.active) this.enable(data);
    else this.layer?.update(data);
  }

  /** Mirror setShowShots: show/hide the custom-rendered shots. Cheap - flips a
   *  draw flag + repaints, no buffer rebuild. */
  setShotsVisible(visible: boolean): void {
    this.shotsVisible = visible;
    this.layer?.setShotsVisible(visible);
    if (this.active) this.map.triggerRepaint();
  }

  /** Mirror setShowStations: show/hide the custom-rendered stations. */
  setStationsVisible(visible: boolean): void {
    this.stationsVisible = visible;
    this.layer?.setStationsVisible(visible);
    if (this.active) this.map.triggerRepaint();
  }

  /** Mirror setShowReferenceShots: hide/show reference shots + all-reference
   *  stations via the shader uniform (instant, like the stock setFilter - no
   *  buffer rebuild). */
  setReferenceShotsVisible(visible: boolean): void {
    this.referenceShotsVisible = visible;
    this.layer?.setReferenceShotsVisible(visible);
    if (this.active) this.map.triggerRepaint();
  }

  /** Mirror setShowJumps: hide/show jump shots independently of references
   *  (#27), same instant shader-uniform path. */
  setJumpShotsVisible(visible: boolean): void {
    this.jumpShotsVisible = visible;
    this.layer?.setJumpShotsVisible(visible);
    if (this.active) this.map.triggerRepaint();
  }

  /** Set the active flag, firing {@link onActiveChange} only on a real change so
   *  SurveyLayer can drop / re-publish the stock bulk sources in lockstep. */
  private setActive(v: boolean): void {
    if (this.active === v) return;
    this.active = v;
    this.onActiveChange?.(v);
  }

  private enable(data: CustomRenderData): void {
    if (!this.map.getStyle) return;
    // addLayer triggers layer.onAdd synchronously; the layer may disable itself
    // there (no WebGL2, shader compile/link failure) and would draw NOTHING.
    const layer = new SurveyCustomLayer(this.layerId, data, (reason) =>
      this.handleSelfDisable(reason),
    );
    // Re-apply display toggles to the fresh layer (a re-enable starts a new
    // SurveyCustomLayer whose flags default to "show everything").
    layer.setShotsVisible(this.shotsVisible);
    layer.setStationsVisible(this.stationsVisible);
    layer.setReferenceShotsVisible(this.referenceShotsVisible);
    layer.setJumpShotsVisible(this.jumpShotsVisible);
    addCustomLayer(this.map, layer, this.beforeId());
    if (layer.isDisabled()) {
      // FATAL: the custom layer cannot draw (no WebGL2 / shader link failure).
      // Custom render is the sole renderer - there is NO stock fallback - so
      // remove the dead layer and route to the error state instead of restoring
      // stock. Warn ungated so the failure is diagnosable in a production build.
      const reason = layer.getDisabledReason() ?? 'unknown';
      console.warn(
        `[customRender] custom layer failed to initialize (${reason}); ` +
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
    // Ungated lifecycle log (one-shot): buffer counts.
    console.info('[customRender] enabled', {
      shotSegments: data.line.segmentCount,
      stations: data.circle.stationCount,
      bytes: data.line.byteLength + data.circle.byteLength,
    });
    this.map.triggerRepaint();
  }

  /** Insert below the interact overlay (the lowest kept layer above the survey
   *  geometry) so the custom geometry sits above loop closures / ghosts and
   *  beneath every hover/selection overlay + labels. Falls back to the top if
   *  absent (unit-test fakes). */
  private beforeId(): string | undefined {
    return this.map.getLayer('survey-interact-shots') ? 'survey-interact-shots' : undefined;
  }

  /** Re-add the layer after MapLibre rebuilt the style without it (context
   *  restore / worker restart, see styleRestore.ts). */
  reattach(): void {
    const layer = this.layer;
    if (!this.active || !layer) return;
    if (!reAddCustomLayer(this.map, layer, this.beforeId())) {
      this.tearDownFatal(layer.getDisabledReason() ?? 'unknown');
      return;
    }
    this.map.triggerRepaint();
  }

  private disable(): void {
    this.teardown();
    // Ungated lifecycle log (one-shot): controller torn down.
    console.info('[customRender] disabled');
  }

  /** Remove the custom layer (the sole survey renderer - no stock to restore).
   *  Safe when already inactive. Non-fatal teardown only (destroy); the fatal
   *  path uses {@link tearDownFatal}. */
  private teardown(): void {
    this.setActive(false);
    removeCustomLayer(this.map, this.layer);
    this.layer = null;
    this.map.triggerRepaint();
  }

  /**
   * The custom layer hit a GL error mid-render and disabled itself. This fires
   * INSIDE MapLibre's frame (from the layer's render()), so mutating the style
   * now is unsafe. Warn ungated with the error code and DEFER the FATAL teardown
   * off the render stack. Idempotent + re-entrancy-safe. There is no stock
   * fallback: the deferred handler routes to {@link tearDownFatal} (error state),
   * not the stock-restoring {@link teardown}.
   */
  private handleSelfDisable(reason: string): void {
    // A lost context fails every GL call; the restore's re-add rebuilds the layer.
    if (!this.active || this.selfDisableScheduled || isStyleRestoring(this.map)) return;
    this.selfDisableScheduled = true;
    console.warn(
      `[customRender] custom layer self-disabled (${reason}); survey rendering unavailable`,
    );
    this.scheduleRestore(() => {
      this.selfDisableScheduled = false;
      if (this.active) this.tearDownFatal(reason);
    });
  }

  /**
   * FATAL teardown: remove the dead layer WITHOUT restoring or republishing the
   * stock layers, latch {@link fatal} so a later sync() bails, and surface the
   * error via {@link onFatal}. Deliberately does NOT fire `onActiveChange(false)`
   * - that would drive SurveyLayer to republish the stock bulk sources, exactly
   * the fallback this stage removes. The survey area stays blank (error state).
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

  /** DEVICE-pixel style sizes for a params set (mirror the stock visual weights).
   *  Shared by build() and the incremental path so a size-only change (no vertex
   *  edit) still reaches the shader uniforms. */
  private sizeFields(params: CustomSyncParams): Pick<
    CustomRenderData,
    | 'lineHalfWidthPx'
    | 'circleRadiusPx'
    | 'circleStrokePx'
    | 'aaPx'
    | 'lineHoverWidthMul'
    | 'lineSelectWidthMul'
    | 'circleHoverSizeMul'
    | 'circleSelectSizeMul'
  > {
    const dpr = (typeof window !== 'undefined' && window.devicePixelRatio) || 1;
    return {
      lineHalfWidthPx: (params.shotWidthPx / 2) * dpr,
      circleRadiusPx: params.stationRadiusPx * dpr,
      circleStrokePx: params.stationStrokePx * dpr,
      aaPx: dpr,
      // Highlight bump ratios (C2), dimensionless (dpr cancels) so they apply to
      // the device-px base sizes above. Absent config falls back to the baked
      // stock ratios so behavior is unchanged without config.
      lineHoverWidthMul: highlightSizeMul(
        params.shotWidthPx, params.shotHoverWidthPx, DEFAULT_SHOT_HOVER_WIDTH_MUL,
      ),
      lineSelectWidthMul: highlightSizeMul(
        params.shotWidthPx, params.shotSelectionWidthPx, DEFAULT_SHOT_SELECT_WIDTH_MUL,
      ),
      circleHoverSizeMul: highlightSizeMul(
        params.stationRadiusPx, params.stationHoverRadiusPx, DEFAULT_STATION_HOVER_SIZE_MUL,
      ),
      circleSelectSizeMul: highlightSizeMul(
        params.stationRadiusPx, params.stationSelectionRadiusPx, DEFAULT_STATION_SELECT_SIZE_MUL,
      ),
    };
  }

  /** Build the GPU-ready buffers from SoA, resolving colors + anchor. */
  private build(params: CustomSyncParams): CustomRenderData {
    const t0 = performance.now();
    const { surveyData, gpsOrigin, colorCtx } = params;
    const [anchorX, anchorY] = computeMercatorAnchor(surveyData, gpsOrigin);
    const stationRel = projectStationsRel(surveyData, gpsOrigin, anchorX, anchorY);
    const lookup = buildStationLookup(surveyData);
    const res = makeColorResolvers(surveyData, colorCtx);
    const allReference = computeStationAllReference(surveyData, lookup);
    // Apply the current dim + drag-hide sets (null on the hot path so the dim /
    // hide attributes stay all-zero without per-item predicate calls).
    const dims = this.hasDimming() ? this.dimPredicates(surveyData) : null;
    const hides = this.hasHide() ? this.hidePredicates(surveyData) : null;
    const highlights = this.hasHighlight() ? this.highlightPredicates(surveyData) : null;
    const line = buildLineBuffers(
      surveyData, stationRel, lookup, res.colorForShot, params.exclude,
      res.useDepthEndpoints ? res.colorForShotEndpoints : undefined,
      dims?.dimForShot, hides?.hideForShot, highlights?.highlightForShot,
    );
    const circle = buildCircleBuffers(
      surveyData, stationRel, res.fillForStation, res.strokeForStation, params.exclude, allReference,
      dims?.dimForStation, hides?.hideForStation, highlights?.highlightForStation,
    );

    const data: CustomRenderData = { anchorX, anchorY, line, circle, ...this.sizeFields(params) };

    const totalBytes = line.byteLength + circle.byteLength;
    debugLog('[customRender] build', {
      ms: +(performance.now() - t0).toFixed(1),
      shotSegments: line.segmentCount,
      stations: circle.stationCount,
      lineMB: +(line.byteLength / 1048576).toFixed(2),
      circleMB: +(circle.byteLength / 1048576).toFixed(2),
      totalMB: +(totalBytes / 1048576).toFixed(2),
    });
    // Cache products so setDimming / setDragHide / the incremental edit path can
    // recompute just the affected arrays in place.
    this.lastData = data;
    this.lastLookup = lookup;
    this.lastSurveyData = surveyData;
    this.lastStationRel = stationRel;
    this.lastAllReference = allReference;
    return data;
  }

  /** True when some anchored section, immovable feature, gas-dim feature, or the
   *  plan-idle tint is currently set. */
  private hasDimming(): boolean {
    return (
      this.dimState.anchoredSectionIds.size > 0 ||
      this.dimState.immovableStationFeatureIds.size > 0 ||
      this.dimState.immovableShotFeatureIds.size > 0 ||
      this.dimState.gasDimStationFeatureIds.size > 0 ||
      this.dimState.gasDimShotFeatureIds.size > 0 ||
      this.dimState.planIdleDim
    );
  }

  /** Per-station / per-shot dim LEVEL predicates for the current {@link dimState}:
   *  a feature dims fully (1) when its section is anchored OR its featureId is
   *  immovable; else it dims at {@link GAS_DIM_LEVEL} (0.5) when its featureId is
   *  beyond the active gas-reach budget OR the plan-idle tint (#395 revision) is
   *  on (every feature dims) - full dim wins on overlap either way. */
  private dimPredicates(surveyData: SurveyData): {
    dimForStation: (g: number) => number;
    dimForShot: (globalShotIdx: number, sectionId: number) => number;
  } {
    const {
      anchoredSectionIds, immovableStationFeatureIds, immovableShotFeatureIds,
      gasDimStationFeatureIds, gasDimShotFeatureIds, planIdleDim,
    } = this.dimState;
    return {
      dimForStation: (g: number): number => {
        const sid = surveyData.stationSectionIds[g]!;
        const fid = sid * FEATURE_ID_STRIDE + surveyData.stationIndices[g]!;
        if (
          anchoredSectionIds.has(sid) ||
          (immovableStationFeatureIds.size > 0 && immovableStationFeatureIds.has(fid))
        ) {
          return 1;
        }
        const gasDim = gasDimStationFeatureIds.size > 0 && gasDimStationFeatureIds.has(fid);
        return gasDim || planIdleDim ? GAS_DIM_LEVEL : 0;
      },
      dimForShot: (globalShotIdx: number, sectionId: number): number => {
        const fid = sectionId * FEATURE_ID_STRIDE + surveyData.shotIds[globalShotIdx]!;
        if (
          anchoredSectionIds.has(sectionId) ||
          (immovableShotFeatureIds.size > 0 && immovableShotFeatureIds.has(fid))
        ) {
          return 1;
        }
        const gasDim = gasDimShotFeatureIds.size > 0 && gasDimShotFeatureIds.has(fid);
        return gasDim || planIdleDim ? GAS_DIM_LEVEL : 0;
      },
    };
  }

  /** True when a drag is hiding some feature (drag active). */
  private hasHide(): boolean {
    return (
      (this.hideState.sectionIds?.size ?? 0) > 0 ||
      (this.hideState.stationGlobalIndices?.size ?? 0) > 0 ||
      (this.hideState.shotGlobalIndices?.size ?? 0) > 0
    );
  }

  /** Per-station / per-shot hide predicates for the current {@link hideState}:
   *  a feature hides when its section, its global station index, or its global
   *  shot index is in the drag-exclusion set. Mirrors the build-time exclusion
   *  skip so a marked vertex collapses exactly where an omitted one would vanish. */
  private hidePredicates(surveyData: SurveyData): {
    hideForStation: (g: number) => boolean;
    hideForShot: (globalShotIdx: number, sectionId: number) => boolean;
  } {
    const { sectionIds, stationGlobalIndices, shotGlobalIndices } = this.hideState;
    return {
      hideForStation: (g: number): boolean =>
        stationGlobalIndices?.has(g) === true ||
        sectionIds?.has(surveyData.stationSectionIds[g]!) === true,
      hideForShot: (globalShotIdx: number, sectionId: number): boolean =>
        shotGlobalIndices?.has(globalShotIdx) === true || sectionIds?.has(sectionId) === true,
    };
  }

  /** True when any emphasis source has a section (hover / selection active). */
  private hasHighlight(): boolean {
    return (
      this.segmentHoverState.size > 0 ||
      this.sectionHoverState.size > 0 ||
      this.sectionSelectState.size > 0
    );
  }

  /** Merged emphasis level for one section: hover (2) if in either hover source,
   *  else selection (1) if selected, else none (0). Hover wins over selection,
   *  matching the stock paint case order (`hover` before `selected`). */
  private highlightLevel(sectionId: number): number {
    if (this.segmentHoverState.has(sectionId) || this.sectionHoverState.has(sectionId)) {
      return HIGHLIGHT_HOVER;
    }
    if (this.sectionSelectState.has(sectionId)) return HIGHLIGHT_SELECT;
    return HIGHLIGHT_NONE;
  }

  /**
   * The size multiplier the circle shader draws a station marker at (radius and
   * stroke both scale by it): a hovered / selected section's markers take the
   * configured highlight size, which wins over the zero-datum 2x, else 1 - the
   * `sizeMul` choice in the circle vertex shader (`shaders.ts`). The station
   * pick sizes the drawn disc with it (`SurveyGeometryIndex.nearestStation`).
   */
  stationSizeMul(sectionId: number, zeroDatum: boolean): number {
    const level = this.highlightLevel(sectionId);
    const d = this.lastData;
    if (level === HIGHLIGHT_HOVER && d) return d.circleHoverSizeMul;
    if (level === HIGHLIGHT_SELECT && d) return d.circleSelectSizeMul;
    return zeroDatum ? 2 : 1;
  }

  /** Per-station / per-shot highlight predicates resolving the merged emphasis
   *  level for a feature's section (0 none, 1 selection, 2 hover). */
  private highlightPredicates(surveyData: SurveyData): {
    highlightForStation: (g: number) => number;
    highlightForShot: (globalShotIdx: number, sectionId: number) => number;
  } {
    return {
      highlightForStation: (g: number): number =>
        this.highlightLevel(surveyData.stationSectionIds[g]!),
      highlightForShot: (_globalShotIdx: number, sectionId: number): number =>
        this.highlightLevel(sectionId),
    };
  }

  /**
   * Update the anchored / immovable dim set (SurveyLayer.setAnchoredSections /
   * setImmovableFeatures). Rare (fires on move-tool activation, not per frame),
   * so it takes the cheap path: recompute ONLY the two per-vertex dim arrays
   * from the cached SoA + exclusion and re-upload just those GPU buffers, no
   * reprojection or color packing. State is stored regardless so the next full
   * build (geometry flush / drag / re-enable) reproduces it.
   */
  setDimming(state: CustomDimState): void {
    this.dimState = state;
    if (!this.active || !this.layer || !this.lastData || !this.lastLookup || !this.lastSurveyData) {
      return;
    }
    const survey = this.lastSurveyData;
    const exclude = this.lastParams?.exclude;
    const { dimForStation, dimForShot } = this.dimPredicates(survey);
    const lineDim = computeLineDim(survey, this.lastLookup, dimForShot, exclude);
    const circleDim = computeCircleDim(survey, dimForStation, exclude);
    // Keep the cached buffers coherent so a later context restore / re-upload
    // (update()) carries the dim state without a fresh setDimming.
    this.lastData.line.dim = lineDim;
    this.lastData.circle.dim = circleDim;
    this.layer.updateDim(lineDim, circleDim);
    this.map.triggerRepaint();
  }

  /**
   * Point the custom layer at (or off, with null) the active drag's hidden
   * feature set (SurveyLayer.setDragExclusion). The cheap drag-begin/end path:
   * the buffers keep every feature, so this recomputes ONLY the two per-vertex
   * hide arrays from the cached SoA and re-uploads just those GPU buffers -
   * no reprojection, color packing, or full rebuild. State is stored regardless
   * so the next full build (mid-drag flush / re-enable) reproduces it.
   */
  setDragHide(exclusion: CustomRenderExclusion | null): void {
    this.hideState = exclusion ?? EMPTY_EXCLUSION;
    if (!this.active || !this.layer || !this.lastData || !this.lastLookup || !this.lastSurveyData) {
      return;
    }
    const survey = this.lastSurveyData;
    // The live drag path never uses a build-time exclusion, so the hide arrays
    // span the full (uncompacted) buffers; computeLine/CircleDim double as the
    // per-vertex flag builders for any predicate.
    const lineHide = this.hasHide()
      ? computeLineDim(survey, this.lastLookup, this.hidePredicates(survey).hideForShot)
      : new Float32Array(this.lastData.line.vertexCount);
    const circleHide = this.hasHide()
      ? computeCircleDim(survey, this.hidePredicates(survey).hideForStation)
      : new Float32Array(this.lastData.circle.vertexCount);
    this.lastData.line.hide = lineHide;
    this.lastData.circle.hide = circleHide;
    this.layer.updateHide(lineHide, circleHide);
    this.map.triggerRepaint();
  }

  /**
   * Emphasize the hovered connected segment (SurveyLayer.setHoveredSegment for
   * the Move Segment tool) at hover level. Pass an empty array to clear (hover
   * leaves geometry / drag begins). See {@link refreshHighlight}.
   */
  setHighlight(sectionIds: readonly number[]): void {
    this.segmentHoverState = sectionIds.length > 0 ? new Set(sectionIds) : EMPTY_FEATURE_SET;
    this.refreshHighlight();
  }

  /**
   * Emphasize the single hovered / panel-row-hovered section at hover level
   * (SurveyLayer.setHoveredSection). Empty array clears. The stock renderer used
   * the same `hover` feature-state the Move-Segment set does, so both feed the
   * hover level here. See {@link refreshHighlight}.
   */
  setSectionHover(sectionIds: readonly number[]): void {
    this.sectionHoverState = sectionIds.length > 0 ? new Set(sectionIds) : EMPTY_FEATURE_SET;
    this.refreshHighlight();
  }

  /**
   * Emphasize the selected section at the (smaller) selection level
   * (SurveyLayer.setSelectedSection). Empty array clears. When a section is both
   * selected and hovered the merge yields hover level (hover wins). See
   * {@link refreshHighlight}.
   */
  setSectionSelect(sectionIds: readonly number[]): void {
    this.sectionSelectState = sectionIds.length > 0 ? new Set(sectionIds) : EMPTY_FEATURE_SET;
    this.refreshHighlight();
  }

  /** Clear every emphasis source (segment hover, section hover, selection) in one
   *  refresh. Cheaper + clearer than three empty setters at a clear-all site. */
  clearHighlights(): void {
    this.segmentHoverState = EMPTY_FEATURE_SET;
    this.sectionHoverState = EMPTY_FEATURE_SET;
    this.sectionSelectState = EMPTY_FEATURE_SET;
    this.refreshHighlight();
  }

  /**
   * Recompute + re-upload the two per-vertex highlight-level arrays from the
   * merged emphasis sources. Sibling of {@link setDimming}: the cheap
   * per-interaction path recomputes ONLY the highlight arrays from the cached SoA
   * (computeLine/CircleDim double as the per-vertex flag builders for any
   * predicate) and re-uploads just those GPU buffers - no reprojection or color
   * packing. State is stored on the source sets regardless so the next full build
   * reproduces it.
   */
  private refreshHighlight(): void {
    if (!this.active || !this.layer || !this.lastData || !this.lastLookup || !this.lastSurveyData) {
      return;
    }
    const survey = this.lastSurveyData;
    const exclude = this.lastParams?.exclude;
    const lineHighlight = this.hasHighlight()
      ? computeLineDim(survey, this.lastLookup, this.highlightPredicates(survey).highlightForShot, exclude)
      : new Float32Array(this.lastData.line.vertexCount);
    const circleHighlight = this.hasHighlight()
      ? computeCircleDim(survey, this.highlightPredicates(survey).highlightForStation, exclude)
      : new Float32Array(this.lastData.circle.vertexCount);
    this.lastData.line.highlight = lineHighlight;
    this.lastData.circle.highlight = circleHighlight;
    this.layer.updateHighlight(lineHighlight, circleHighlight);
    this.map.triggerRepaint();
  }

  /**
   * Try to apply an edit incrementally (only the changed sections' vertex
   * ranges) instead of a full rebuild. Returns false to fall back to build().
   * Requires an active layer with cached products; `planIncrementalUpdate` gates
   * on layout / color-global / anchor / gps stability (see incremental.ts).
   */
  private tryIncremental(prevParams: CustomSyncParams, params: CustomSyncParams): boolean {
    const prev = this.lastSurveyData;
    const layer = this.layer;
    const line = this.lastData?.line;
    const circle = this.lastData?.circle;
    const rel = this.lastStationRel;
    const allRef = this.lastAllReference;
    const lookup = this.lastLookup;
    if (!prev || !layer || !this.lastData || !line || !circle || !rel || !allRef || !lookup) return false;
    // A build-time exclusion (tests) compacts the buffers, breaking the g↦[4g]
    // station map the incremental path relies on; only the live (full) path is
    // eligible.
    if (prevParams.exclude || params.exclude) return false;
    const next = params.surveyData;
    const plan = planIncrementalUpdate(
      prev, next, prevParams.colorCtx, params.colorCtx,
      prevParams.gpsOrigin, params.gpsOrigin, this.lastData.anchorX, this.lastData.anchorY,
    );
    if (!plan) return false;

    // Reproject moved stations in place (same anchor by the plan's gate).
    const { anchorX, anchorY } = this.lastData;
    for (const g of plan.reprojectStationGlobals) {
      const ll = metersToMapCoords(next.stationCoords[g * 3]!, next.stationCoords[g * 3 + 1]!, params.gpsOrigin);
      const [mx, my] = lngLatToMercator(ll[0], ll[1]);
      rel[g * 2] = mx - anchorX;
      rel[g * 2 + 1] = my - anchorY;
    }

    const res = makeColorResolvers(next, params.colorCtx);
    const dims = this.hasDimming() ? this.dimPredicates(next) : null;
    const hides = this.hasHide() ? this.hidePredicates(next) : null;
    const highlights = this.hasHighlight() ? this.highlightPredicates(next) : null;
    const lineInputs: LineSectionInputs = {
      colorForShot: res.colorForShot,
      colorForShotEndpoints: res.useDepthEndpoints ? res.colorForShotEndpoints : undefined,
      dimForShot: dims?.dimForShot,
      hideForShot: hides?.hideForShot,
      highlightForShot: highlights?.highlightForShot,
    };
    const circleInputs: CircleStationInputs = {
      fillForStation: res.fillForStation,
      strokeForStation: res.strokeForStation,
      dimForStation: dims?.dimForStation,
      hideForStation: hides?.hideForStation,
      highlightForStation: highlights?.highlightForStation,
    };

    // Re-emit each dirty section's line range + its stations (all-reference
    // first, since a station read depends on it), collecting station slots.
    // A kind edit can move a station between ranks, which reorders the circle
    // draw (real-line markers on top): note it, and redo the index buffer below.
    const circleSlots: number[] = [];
    let rankMoved = false;
    for (const si of plan.dirtySectionIdxs) {
      const section = next.sections[si]!;
      const inner = lookup.get(section.id);
      // A station's kind mask reads only its section's shot kinds (the layout is
      // fixed by the plan's gate), so only a kind edit can move a rank: snapshot
      // the ranks just then, not on every position edit.
      let kindsEdited = false;
      for (let i = section.shotOffset; i < section.shotOffset + section.shotCount; i++) {
        if (prev.shotFlags[i] !== next.shotFlags[i]) {
          kindsEdited = true;
          break;
        }
      }
      const rankBefore = kindsEdited && inner ? Array.from(inner.values(), (g) => stationKindRank(allRef[g]!)) : null;
      recomputeSectionAllReference(next, lookup, section, allRef);
      if (inner) {
        let k = 0;
        for (const g of inner.values()) {
          if (rankBefore && stationKindRank(allRef[g]!) !== rankBefore[k++]) rankMoved = true;
          recomputeStation(circle, g, next, rel, circleInputs, allRef[g]!);
          circleSlots.push(g);
        }
      }
      recomputeLineSection(line, si, next, rel, lookup, lineInputs);
    }
    if (rankMoved && writeCircleDrawOrder(circle)) layer.uploadCircleIndices();

    // Sub-upload: one contiguous line range per section; coalesced station runs.
    for (const si of plan.dirtySectionIdxs) {
      const segStart = line.sectionRanges[si * 2]!;
      const segCount = line.sectionRanges[si * 2 + 1]!;
      layer.uploadLineVertexRange(segStart * 4, segCount * 4);
    }
    circleSlots.sort((a, b) => a - b);
    for (const [vStart, vCount] of coalesceVertexRuns(circleSlots, 4)) {
      layer.uploadCircleVertexRange(vStart, vCount);
    }

    // Refresh caches + size uniforms (a size-only change needs no upload).
    Object.assign(this.lastData, this.sizeFields(params));
    this.lastSurveyData = next;
    this.map.triggerRepaint();
    return true;
  }

  /**
   * Try to apply a pure global recolor (analytical-overlay enter/exit, theme
   * change, depth / severity toggle) as a color-only re-upload instead of a full
   * rebuild. The geometry is unchanged, so this recomputes ONLY the four color
   * arrays via the shared resolvers (in the SAME SoA order the full build wrote
   * them, so the bytes are identical to a rebuild) and re-uploads just the two
   * color buffers. Returns false to fall back to build() when the change is not
   * color-only (see {@link planColorOnlyUpdate}) or a build-time exclusion is
   * active (compaction breaks the g↦[4g] map, as for {@link tryIncremental}).
   */
  private tryColorOnly(prevParams: CustomSyncParams, params: CustomSyncParams): boolean {
    const prev = this.lastSurveyData;
    const layer = this.layer;
    const line = this.lastData?.line;
    const circle = this.lastData?.circle;
    const lookup = this.lastLookup;
    if (!prev || !layer || !this.lastData || !line || !circle || !lookup) return false;
    if (prevParams.exclude || params.exclude) return false;
    const next = params.surveyData;
    if (
      !planColorOnlyUpdate(
        prev, next, prevParams.colorCtx, params.colorCtx,
        prevParams.gpsOrigin, params.gpsOrigin, this.lastData.anchorX, this.lastData.anchorY,
      )
    ) {
      return false;
    }

    const res = makeColorResolvers(next, params.colorCtx);
    recomputeLineColors(
      line, next, lookup, res.colorForShot,
      res.useDepthEndpoints ? res.colorForShotEndpoints : undefined,
    );
    recomputeCircleColors(circle, next, res.fillForStation, res.strokeForStation);
    layer.updateColors(line.colorFrom, line.colorTo, circle.fill, circle.stroke);

    // Cache coherence: geometry buffers are untouched; refresh the survey ref +
    // size uniforms (a size-only change rides here with no extra upload).
    Object.assign(this.lastData, this.sizeFields(params));
    this.lastSurveyData = next;
    this.map.triggerRepaint();
    return true;
  }

  private installHook(): void {
    if (this.hookInstalled || typeof window === 'undefined') return;
    (window as unknown as Record<string, unknown>)[CUSTOM_RENDER_SYNC_HOOK] = () =>
      this.runSyncHook();
    this.hookInstalled = true;
  }

  /**
   * Body of the runtime `__stalagmightSyncCustomRender` hook. Prefer a
   * fresh-geometry resync (SurveyLayer reads the store) so a manual re-sync
   * after a project switch never rebuilds from a stale `lastParams` snapshot;
   * fall back to the snapshot only when no resync was wired (unit tests).
   */
  private runSyncHook(): void {
    if (this.disposed) return;
    if (this.resync) this.resync();
    else if (this.lastParams) this.sync(this.lastParams);
  }

  /**
   * Lightweight release WITHOUT touching the map (no removeLayer, no stock
   * republish). Called on MapCanvas unmount via SurveyLayer.releaseVtRegistry(),
   * which deliberately skips the full {@link destroy} for unmount speed. Without
   * this the window sync hook would pin this controller (and its cached
   * SurveyData + GL buffers) alive until the next SurveyLayer overwrites the
   * global -> hundreds of MB retained on large projects (#mem). Uninstalls the
   * hook, sets the {@link disposed} guard so a late hook fire / sync bails, and
   * nulls the heavy caches. `map.remove()` handles the layer's GL teardown, so a
   * republish here would be wasted work. Idempotent.
   */
  dispose(): void {
    this.disposed = true;
    if (this.hookInstalled && typeof window !== 'undefined') {
      delete (window as unknown as Record<string, unknown>)[CUSTOM_RENDER_SYNC_HOOK];
      this.hookInstalled = false;
    }
    this.lastParams = null;
    this.lastData = null;
    this.lastLookup = null;
    this.lastSurveyData = null;
    this.lastStationRel = null;
    this.lastAllReference = null;
  }

  destroy(): void {
    if (this.active) this.disable();
    // Reuse dispose() for the hook removal + cache clearing (DRY); the active-layer
    // teardown above is destroy()'s addition over the unmount-lite dispose() path.
    this.dispose();
  }
}
