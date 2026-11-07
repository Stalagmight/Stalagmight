import type { Map as MaplibreMap, MapMouseEvent } from 'maplibre-gl';
import type * as maplibregl from 'maplibre-gl';
import type { LoopClosure, Section, SectionId, ShotId, Station, StationRef } from '../../types/project';
import type { Config, GpsOrigin, DecimalPrecision } from '../../types/config';
import type { RouteLeg } from '../../types/ipc.generated';
import { isNonInteractiveTool } from '../../utils/toolUtils';
import type { ToolType } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { selectActiveSurveyData } from '../../store/geometrySelectors';
import { surveyDataFromLegacy } from '../../types/survey-data';
import { canWriteStyle, hasLiveStyle } from './mapUtils';
import { mapCursorFor } from './mapCursor';
import {
  normalizeDepthRange,
  type DepthRange,
  type DisplayColorContext,
  type GhostSectionData,
} from './surveyLayer/displayColor';
import {
  SurveyGeometryIndex,
  type VisibleKinds,
} from './surveyLayer/geometryIndex';
import {
  type DragPreviewSet,
  type Projector,
} from './surveyLayer/dragPreviewSource';
import type { DragPreviewLayerCtx } from './surveyLayer/dragPreviewLayers';
import {
  shotOverlayFeature,
  stationOverlayFeature,
  type InteractOverlayDeps,
} from './surveyLayer/interactOverlayFeatures';
import { DragGhostController } from './surveyLayer/dragGhostController';
import { DragPreviewController } from './surveyLayer/dragPreviewController';
import type { GhostRenderData } from './surveyLayer/customRender/surveyGhostLayer';
import { buildGhostRenderData } from './surveyLayer/dragGhostData';
import { SurveyHitTester } from './surveyLayer/surveyHitTester';
import { computeIssueSeverity, type IssueSeverity } from './surveyLayer/severity';
import { SurveyCustomController, buildCustomDimState } from './surveyLayer/customRender/controller';
import { SurveyLabelController } from './surveyLayer/customRender/labelController';
import { MeasurePathController } from './surveyLayer/customRender/measurePathController';
import { isStyleRestoring } from './surveyLayer/customRender/styleRestore';
import { syncMeasurePath } from './surveyLayer/measurePathSync';
import { trackMoveEndNav, trackZoomEndNav } from './surveyLayer/navTelemetry';
import type { SurveyData } from '../../types/survey-data';
import type { ProjectWarning } from '../../types/commands';
import { getMapThemeColors, type MapThemeColors } from '../../utils/themeColors';
import type { DistanceFeatureState } from '../../utils/distanceGradient';
import type { GasReachFeatureState } from '../../utils/gasReach';
import { FEATURE_ID_STRIDE, sectionIdFromKey } from '../../utils/ids';
import { MEASURE_TINT } from '../../utils/measurePath';
import { watchDevicePixelRatio } from '../../utils/dprWatcher';
import { attachLongPressTouch } from '../../utils/longPressTouch';
import { shouldSuppressMapError, type MapErrorEventLike } from './surveyLayer/mapErrors';
import { diffRemovedSectionIds } from './surveyLayer/sectionDiff';
import { InteractOverlayHideController, overlayPaintDigest, overlayTouchesRemovedSections } from './surveyLayer/interactOverlayHide';
import { HoverFrame } from './surveyLayer/hoverFrame';
import { watchGlContextLoss } from './surveyLayer/glContextRecovery';
import type { SemanticMapTarget } from './surveyLayer/semanticTarget';
import { SemanticTargetPreference } from './surveyLayer/semanticTargetPreference';
import {
  AUX_IDS,
  addAuxiliaryLayers,
  addAuxiliarySources,
  applyAuxiliaryThemeColors,
  removeAuxiliaryLayers,
  updateGhostDebugVisualization,
  updateGhostHitboxLayers,
  updateGhostVisualLayers,
} from './surveyLayer/auxiliaryLayers';
import {
  applyClosureOpacity,
  applyGasRoute,
  applyGhostSection,
  publishLoopClosures,
} from './surveyLayer/auxiliaryData';
import { AnalyticalOverlays } from './surveyLayer/analyticalOverlays';

/** Whether replacing dim set `prev` with `next` changes nothing. */
function sameOrBothEmpty(next: ReadonlySet<number>, prev: ReadonlySet<number>): boolean {
  return next === prev || (next.size === 0 && prev.size === 0);
}

/** flushUpdate's arguments, in order. */
type FlushArgs = [
  sections: Section[],
  stations: Station[],
  loopClosures: LoopClosure[] | undefined,
  gpsOrigin: GpsOrigin | null,
  showDepthGradient: boolean,
  depthRange: DepthRange,
  precision: DecimalPrecision | undefined,
  showIssueSeverity: boolean,
  warnings: ProjectWarning[],
];

export interface SurveyLayerCallbacks {
  onStationClick?: (ref: StationRef, event: MapMouseEvent) => void;
  onShotClick?: (sectionId: SectionId, shotId: ShotId, event: MapMouseEvent) => void;
  onEmptyClick?: (event: MapMouseEvent) => void;
  onStationHover?: (ref: StationRef | null) => void;
  onShotHover?: (shot: { sectionId: SectionId; shotId: ShotId } | null) => void;
  onFeatureHover?: ((isHovering: boolean) => void) | undefined;
  /** `alongT` is where on the shot the click landed, 0..1 from the from-station.
   *  A shot is a straight 3D segment, so the fraction along its planimetric
   *  projection equals the fraction along its slope length: callers multiply
   *  straight through by `shot.length`. */
  onContextMenu?: (
    target: { type: 'station'; ref: StationRef }
      | { type: 'shot'; sectionId: SectionId; shotId: ShotId; alongT: number } | null,
    event: MapMouseEvent
  ) => void;
  /** Called when user clicks near a hidden station (for toast feedback) */
  onHiddenStationClick?: () => void;
  /** Called when user clicks near a hidden shot (for toast feedback) */
  onHiddenShotClick?: () => void;
  /** Called when the custom WebGL renderer hits an unrecoverable GL failure
   *  (WebGL2/GPU). The custom layer is the sole survey renderer with no stock
   *  fallback, so the survey area is now blank - surface a user-facing error. */
  onRenderFatal?: (reason: string) => void;
}

/**
 * Survey layer that renders shots and stations on the map
 *
 * This is a simplified implementation using MapLibre GL JS.
 * For better performance with large datasets, this could be
 * replaced with deck.gl layers.
 */
export type { GhostSectionData } from './surveyLayer/displayColor';

export default class SurveyLayer {
  private map: MaplibreMap;
  private callbacks: SurveyLayerCallbacks;
  /** Candidate-section + nearest-feature hit-testing; see surveyLayer/surveyHitTester.ts. */
  private readonly hits: SurveyHitTester;
  private semanticTargetPreference = new SemanticTargetPreference({ geometry: () => this.enumerateGeometry(), project: (lngLat) => this.map.project(lngLat), visibility: () => ({ stations: this.stationsVisible, shots: this.shotsVisible }), callbacks: () => this.callbacks });

  // Config storage with fallback defaults for when config is null (initial load)
  private config: Config | null = null;
  private static readonly DEFAULTS = {
    station: { radius: 4, hit_area: 10, hover_radius: 6, selection_radius: 5 },
    shot: { width: 2, hit_area: 8, hover_width: 4, selection_width: 3 },
  };

  // Visual state
  private selectedStationId: string | null = null;
  private selectedShotId: string | null = null;
  private hoveredStationId: string | null = null;
  private hoveredShotId: string | null = null;
  private snappedStationId: string | null = null;
  private hoveredSectionId: string | null = null;
  private selectedSectionId: string | null = null;
  private highlightedStationIds: string[] = [];
  // Multi-section hover for Move Segment tool (highlights all connected sections)
  private hoveredSegmentSectionIds: string[] = [];

  // Cached reference-shots visibility (custom renderer + labels + measure path
  // read it to hide/show reference legs).
  private showReferenceShotsCached = true;
  /** #27: jumps hide independently of references. */
  private showJumpsCached = true;

  // Comment tool override: when comment/move_comment tool is active, force both
  // comment layers visible with text-allow-overlap so comments win over other labels.
  private commentToolActive = false;
  private cachedShowStationComments = false;
  private cachedShowShotComments = false;

  // Active tool state (for disabling interactions when non-interactive tools are active)
  private activeTool: ToolType = 'select';

  // Anchored section IDs (for fading when Move Segment tool is active)
  private anchoredSectionIds: ReadonlySet<number> = new Set();

  // Immovable feature IDs for Move Station / Move Shot tools
  // (per-feature granularity, unlike anchoredSectionIds which is per-section)
  private immovableStationFeatureIds: ReadonlySet<number> = new Set();
  private immovableShotFeatureIds: ReadonlySet<number> = new Set();

  // When MapCanvas last confirmed a placement (performance.now() ms); see
  // modalInteractionActive for why the store read alone cannot see it.
  private lastPlacementConfirmAt = -Infinity;
  // One-shot callback fired at end of update() - used to restore filters after
  // source data has been updated, in the same synchronous block (no paint gap).
  private onNextUpdateCallback: (() => void) | null = null;

  /**
   * setData coalescer: if multiple update() calls land in the same microtask
   * (typical of React effect cascades - depthRange/gpsOrigin/precision refs
   * changing in lockstep on project load), we batch them into a single
   * buildSurveyFeatures + setData pass. This was measured at 6× call count
   * per survey load before coalescing; collapsing it ~6× the GC churn.
   *
   * Microtask boundary matters: useLayoutEffect callers expect setData to
   * have run before paint, and microtasks drain before paint. A later
   * deadline (rAF / setTimeout) would break drag-commit's same-frame
   * guarantee documented on the useLayoutEffect site in MapCanvas.
   */
  private pendingUpdateArgs: {
    sections: Section[];
    stations: Station[];
    loopClosures: LoopClosure[] | undefined;
    gpsOrigin: GpsOrigin | null;
    showDepthGradient: boolean;
    depthRange: DepthRange;
    precision: DecimalPrecision | undefined;
    showIssueSeverity: boolean;
    warnings: ProjectWarning[];
  } | null = null;
  private updateScheduled = false;
  /** The latest flush that landed while the map had no style (a lost WebGL
   *  context) or a rebuilt one was loading, replayed once the custom layers are
   *  back (#576 D38). */
  private updateWhileStyleless: FlushArgs | null = null;
  /** Detach for the WebGL context-loss recovery (glContextRecovery.ts). */
  private detachGlContextWatch: (() => void) | null = null;
  /** Coalesced-call counter, surfaced to memory probe for instrumentation. */
  private coalescedUpdateCallsSinceLastFlush = 0;
  /** Drag ghost (#mem drag latency): owns the moving-geometry preview and drives
   *  it from a live screen-px offset. The CUSTOM path renders a WebGL layer whose
   *  drag delta is a single vertex-shader uniform (no worker, no re-bake, so no
   *  overshoot); the STOCK fallback is the historical GeoJSON preview +
   *  single-flight {@link GhostFollow} bake / `line-translate` machine. See
   *  {@link DragGhostController}. Constructed once, reused across drags. */
  private dragGhost!: DragGhostController;
  /** The drag-preview session (capture, exclusion, overlay follow, commit / end);
   *  see surveyLayer/dragPreviewController.ts. */
  private readonly drag: DragPreviewController;
  /** Hides the interact-overlay layers across a removal flush's async re-tile so a
   *  deleted feature's hover/selection highlight drops in the geometry frame
   *  instead of lingering on the stale GeoJSON tile. See interactOverlayHide.ts. */
  private readonly interactHide: InteractOverlayHideController;

  // Visibility state (for hidden feature detection)
  private stationsVisible: boolean = true;
  private shotsVisible: boolean = true;
  /** The merge-preview ghost section (setGhostSection), kept for the context-restore replay. */
  private ghostSection: GhostSectionData | null = null;

  // GPS origin for coordinate transforms (enables satellite tile alignment)
  private gpsOrigin: GpsOrigin | null = null;

  // Depth gradient mode state (used during update() for feature color calculation)
  private showDepthGradient: boolean = false;
  private depthRange: DepthRange = { min: 0, max: 50 };

  // Issue-severity coloring mode (#22). Like showDepthGradient, cached so the
  // lazy-labels zoom rebuild and theme rebuild re-fire with the same mode.
  private showIssueSeverity: boolean = false;
  private cachedWarnings: ProjectWarning[] = [];

  // Analytical-overlay modes: distance gradient (#34), gas reachable frontier
  // (#38) and the Plan-mode idle tint (#395 revision). See analyticalOverlays.ts.
  private readonly overlays = new AnalyticalOverlays();
  // Gas route highlight (#233): the current picked-route legs. Re-applied after every source
  // rebuild (its own GeoJSON source survives setData, but a feature rebuild re-pushes the data).
  private gasRouteLegs: (RouteLeg & { color: string })[] = [];
  // Measured along-line path (#26): the path's shots AND stations, tinted in a
  // user-configurable color by the dedicated custom measure-path layer.
  private measuredPathIds: Set<number> | null = null;
  private measuredPathStationIds: Set<number> | null = null;
  private measureTintColor: string = MEASURE_TINT;

  // Theme colors for MapLibre paint properties (MapLibre cannot read CSS vars)
  // Seeded at construction so first paint uses current theme, not legacy fallbacks
  private themeColors: MapThemeColors = getMapThemeColors();

  // Cached input data for the theme rebuild / drag-preview / hit-test paths.
  private cachedSections: Section[] = [];
  private cachedStations: Station[] = [];
  private cachedLoopClosures: LoopClosure[] | undefined;
  private cachedPrecision: DecimalPrecision | undefined;

  // Bound event handlers stored for proper cleanup in destroy()
  private boundMouseMove: ((e: maplibregl.MapMouseEvent) => void) | null = null;
  private boundMapLeave: (() => void) | null = null;

  // Touch long-press -> context menu. The state machine (500 ms hold, 10 px
  // cancel, swallowed touchend) is shared with GlobalSurveyLayer; see
  // src/utils/longPressTouch.ts.
  private detachLongPressTouch: (() => void) | null = null;

  // Hover hit-testing, coalesced to the latest mouse position per frame.
  private hoverFrame: HoverFrame<maplibregl.MapMouseEvent> | null = null;

  // Interact-overlay dedupe key: per-frame hover updates only touch the
  // overlay source when its content actually changes. Reset to '' whenever
  // styling inputs change (rebuild, config, theme, overlay modes).
  private lastInteractKey: string | null = null;
  /** {@link overlayPaintDigest} of the features the last overlay build emitted. */
  private lastOverlayDigest = '';

  // SoA survey data + geometry resolver for the last applied build (#mem).
  // The resolver serves per-item hit-testing over merged features and the
  // interact overlay's geometry/color lookups. `cachedIssueSeverity` mirrors
  // the severity arrays the build used so overlay colors match exactly.
  private cachedSurveyData: SurveyData | null = null;
  private geoIndex: SurveyGeometryIndex | null = null;
  private cachedIssueSeverity: IssueSeverity | undefined;

  // Flag-gated custom WebGL render prototype (#customRender). No-op unless the
  // `stalagmight_custom_render` gate is on; see surveyLayer/customRender/.
  private customRender: SurveyCustomController | null = null;
  // Flag-gated custom WebGL LABEL layer (#customRender labels). No-op unless BOTH
  // the render gate AND `stalagmight_custom_labels` are on. Draws all five label
  // kinds, VIEWPORT-SCOPED with screen-space collision, rebuilding on camera
  // settle (its own moveend/zoomend listener); suppresses the five stock text
  // layers while active.
  private customLabels: SurveyLabelController | null = null;

  // Dedicated custom WebGL layer for the measured along-line ruler tint (#26,
  // stage 2a of the stock deletion). Draws the measured shots + stations in
  // MEASURE_TINT independently of the stock `survey-measure-path` layer / the
  // `-shots`/`-stations` VT sources. Driven by setMeasuredPath; runs alongside
  // the stock feature-state tint this stage (identical pixels), so stage 2b can
  // delete the stock path.
  private measurePath: MeasurePathController | null = null;
  /** Dispose for the device-pixel-ratio watcher (custom layers bake dpr). */
  private dprWatcherDispose: (() => void) | undefined;

  constructor(map: MaplibreMap, callbacks: SurveyLayerCallbacks = {}) {
    this.map = map;
    this.callbacks = callbacks;
    this.hits = new SurveyHitTester(map, {
      geoIndex: () => this.geoIndex,
      stationsVisible: () => this.stationsVisible,
      shotsVisible: () => this.shotsVisible,
      visibleKinds: () => this.visibleKinds(),
      stationHitReach: () => this.getStationHitReach(),
      shotHitReach: () => this.getShotHitReach(),
    });
    this.interactHide = new InteractOverlayHideController(map, AUX_IDS.interactSourceId, [
      AUX_IDS.interactShotsLayerId,
      AUX_IDS.interactShotsMeasuredLayerId,
      AUX_IDS.interactStationsLayerId,
      AUX_IDS.stationsHighlightLayerId,
    ], () => hasLiveStyle(map));
    addAuxiliarySources(this.map);
    addAuxiliaryLayers(this.map, {
      shotHitArea: this.getShotHitArea(),
      stationHitArea: this.getStationHitArea(),
      shotWidth: this.getShotWidth(),
      stationRadius: this.getStationRadius(),
      themeColors: this.themeColors,
    });
    this.initializeEventHandlers();
    this.customRender = new SurveyCustomController(map, undefined, {
      resync: () => this.resyncCustomRenderFromStore(),
      onFatal: (reason) => this.handleRenderFatal(reason),
    });
    // resyncCustomRenderFromStore syncs BOTH controllers, so one hook covers both.
    this.customLabels = new SurveyLabelController(map, undefined, {
      resync: () => this.resyncCustomRenderFromStore(),
      onFatal: (reason) => this.handleRenderFatal(reason),
    });
    this.dragGhost = new DragGhostController(map, {
      ghostData: (set) => this.dragGhostData(set),
      layerCtx: () => this.dragPreviewLayerCtx(),
      projector: () => this.dragProjector(),
      dpr: () => (typeof window !== 'undefined' && window.devicePixelRatio) || 1,
      scheduleRestore: (fn) =>
        typeof requestAnimationFrame === 'function' ? requestAnimationFrame(fn) : setTimeout(fn, 0),
    });
    this.drag = new DragPreviewController(map, this.dragGhost, this.overlays, {
      geoIndex: () => this.geoIndex,
      surveyData: () => this.cachedSurveyData,
      sections: () => this.cachedSections,
      loopClosures: () => this.cachedLoopClosures,
      gpsOrigin: () => this.gpsOrigin,
      displayColorContext: () => this.displayColorContext(),
      themeColors: () => this.themeColors,
      parseFeatureId: (id) => this.parseFeatureId(id),
      customRender: () => this.customRender,
      customLabels: () => this.customLabels,
      refreshInteractOverlay: () => this.refreshInteractOverlay(),
      invalidateInteractOverlay: () => {
        this.lastInteractKey = null;
      },
    });
    this.measurePath = new MeasurePathController(map, {
      onFatal: (reason) => this.handleRenderFatal(reason),
    });
    // The custom layers bake devicePixelRatio into their device-pixel sizes, so a
    // DPR change (window dragged to a different-scale monitor) must rebuild them.
    this.dprWatcherDispose = watchDevicePixelRatio(() => this.handleDprChange());
    // MapLibre drops every custom layer when it rebuilds the style (a WebGL
    // context loss or a worker restart) and never re-adds them; bring them back
    // once the rebuilt style has loaded (#576 D38, #606). Bottom to top: the
    // order a layer with no place in the layer-order snapshot goes back in.
    this.detachGlContextWatch = watchGlContextLoss(map, {
      layers: () =>
        [this.customRender, this.measurePath, this.customLabels, this.dragGhost].filter(
          (c): c is NonNullable<typeof c> => c !== null,
        ),
      // Hidden overlay layers would be serialized hidden into the restored style.
      beforeLoss: () => this.interactHide.restore(),
      afterRestore: () => this.replayAfterContextRestore(),
    });
  }

  /**
   * After a style rebuild the custom layers are back with the data they held at
   * the loss, but anything that arrived meanwhile bailed on the missing style or
   * was held back while the rebuilt one loaded. Re-run the last flush - the one
   * the window swallowed, if any - so geometry, labels, the measured path and the
   * overlays match the store again.
   */
  private replayAfterContextRestore(): void {
    const args: FlushArgs | null =
      this.updateWhileStyleless ??
      (this.cachedSections.length > 0 || this.cachedStations.length > 0
        ? [this.cachedSections, this.cachedStations, this.cachedLoopClosures, this.gpsOrigin,
          this.showDepthGradient, this.depthRange, this.cachedPrecision, this.showIssueSeverity,
          this.cachedWarnings]
        : null);
    this.updateWhileStyleless = null;
    if (args) this.flushUpdateSync(...args);
    // The closure filter a carry cancelled during the loss could not put back, and
    // the closure exclusion of a carry picked up meanwhile could not write.
    this.drag.replayAfterContextRestore();
    // Closure dimming is a paint property the lost window could not write.
    this.updateClosureOpacity();
    // So is the gas dim a setGasReach during the loss stored but could not apply (it
    // bails without a style), and an unchanged later call no longer re-applies it
    // (#576 NF-W13FE-1).
    this.syncCustomDimming();
    // So are the rest of the stock layers' style state: the restored style is the
    // one serialized at the loss, and the setters below skipped their writes
    // while it was gone (#576 NF-MAP-5).
    this.applyStockVisibility();
    this.applyConfigPaint();
    applyAuxiliaryThemeColors(this.map, this.themeColors);
    this.applyInteractPaint();
    applyGhostSection(this.map, this.ghostSection, this.gpsOrigin);
    // Unconditionally, unlike the flush's re-push: a route cleared during the loss
    // (leaving Plan, the field going away) is still in the restored source - a
    // GeoJSON source serializes its data - and only an empty push erases it
    // (#576 RV4-FE-SL3).
    applyGasRoute(this.map, this.gasRouteLegs, this.cachedStations, this.gpsOrigin);
    this.lastInteractKey = null;
    this.refreshInteractOverlay();
    // A measured path set or cleared while the style was gone never reached the
    // measure-path layer (its sync bails without a style).
    this.syncCustomMeasurePath();
  }

  /**
   * A device-pixel-ratio change invalidated the baked device-pixel sizes of the
   * custom geometry, label, and measure-path layers; rebuild them from the store
   * (which re-reads the live dpr). The interact overlay is stock MapLibre and
   * scales with dpr on its own, so it needs no rebuild here.
   */
  private handleDprChange(): void {
    this.resyncCustomRenderFromStore(); // geometry + labels
    this.syncCustomMeasurePath(); // measure-path tint
  }

  setCallbacks(callbacks: SurveyLayerCallbacks) {
    this.callbacks = callbacks;
  }

  /**
   * A custom WebGL renderer subsystem (geometry, labels, or the measure-path
   * tint) hit an unrecoverable failure and removed its dead layer. There is no
   * stock fallback, so the affected content is gone until reload. Log ungated
   * (diagnosable in a production build) and notify the app so it can surface a
   * user-facing error; the `reason` carries the subsystem so the toast can be
   * specific. Read at call time, so a later setCallbacks still routes here.
   */
  private handleRenderFatal(reason: string): void {
    console.error(`[customRender] rendering fault: ${reason}`);
    this.callbacks.onRenderFatal?.(reason);
  }

  // Config setter - triggers layer updates when config changes
  public setConfig(config: Config): void {
    this.config = config;
    this.applyConfigPaint();
    // Overlay features carry config-derived widths/radii - force a rebuild.
    this.lastInteractKey = null;
    this.updateVisualState();
  }

  /** Config-derived paint of the stock ghost layers. Skipped while the WebGL
   *  context is lost (no style); replayAfterContextRestore re-applies it. */
  private applyConfigPaint(): void {
    if (!hasLiveStyle(this.map)) return;
    updateGhostHitboxLayers(this.map, this.getStationHitArea(), this.getShotHitArea());
    updateGhostVisualLayers(this.map, this.getStationRadius(), this.getShotWidth());
    updateGhostDebugVisualization(this.map, this.config?.debug?.map?.show_hitboxes ?? false);
  }

  /**
   * Set the active tool. When switching to a non-interactive tool (pan/zoom),
   * clears hover state and notifies callbacks to sync MapCanvas state.
   * Self-healing: also clears stale hover if already in a non-interactive tool.
   */
  public setActiveTool(tool: ToolType): void {
    const wasMoveComment = this.activeTool === 'move_comment';
    this.activeTool = tool;

    const isNonInteractive = isNonInteractiveTool(tool);

    // Clear hover when in non-interactive tool AND there's stale hover state
    // This is self-healing: handles both transitions (interactive→non-interactive)
    // and missed initial syncs or future refactors
    if (isNonInteractive && (this.hoveredStationId || this.hoveredShotId || this.hoveredSectionId)) {
      this.hoveredStationId = null;
      this.hoveredShotId = null;
      this.hoveredSectionId = null;
      this.customRender?.setSectionHover([]);
      this.updateVisualState();

      // CRITICAL: Also notify callbacks to clear MapCanvas state
      this.callbacks.onFeatureHover?.(false);
      this.callbacks.onStationHover?.(null);
      this.callbacks.onShotHover?.(null);
    }

    // Re-derive the hover verdict from the state just updated above (the tool
    // decides which hovered features may show the pointer). The cursor itself is
    // the owner's (mapCursor.ts): literal writes here used to wipe the tool cursor
    // MapCanvas had set for exactly the tools (pan, zoom) whose cursor IS the
    // affordance (#24).
    this.refreshHoverCursor();

    // Comment tool override: force comment layers visible + overlap priority
    const isCommentTool = tool === 'comment' || tool === 'move_comment';
    if (isCommentTool && !this.commentToolActive) {
      this._enableCommentToolOverride();
    } else if (!isCommentTool && this.commentToolActive) {
      this._disableCommentToolOverride();
    }

    // Move-comment palette tint (#52): exactly `move_comment` (not the plain
    // `comment` tool) tints each comment label and its owning shot/station.
    // Re-derive the paint expressions only on transition in/out.
    const isMoveComment = tool === 'move_comment';
    if (isMoveComment !== wasMoveComment) {
      this.applyOverlayPaint();
      // Custom renderer + labels (no feature-state / commentColor property): rebuild
      // so the shot/station/comment tint appears or clears (commentTint in sync()).
      this.resyncCustomRenderFromStore();
    }
  }

  // Helper methods that use fallback defaults when config is null
  private getStationRadius(): number {
    return this.config?.map?.station?.radius ?? SurveyLayer.DEFAULTS.station.radius;
  }

  private getStationHitArea(): number {
    return this.config?.map?.station?.hit_area ?? SurveyLayer.DEFAULTS.station.hit_area;
  }

  private getShotWidth(): number {
    return this.config?.map?.shot?.width ?? SurveyLayer.DEFAULTS.shot.width;
  }

  private getShotHitArea(): number {
    return this.config?.map?.shot?.hit_area ?? SurveyLayer.DEFAULTS.shot.hit_area;
  }

  /**
   * Screen-space "reach" of the (now-removed) hitbox layers - the maximum px
   * distance from a geometry's center at which a point still registered a hit,
   * i.e. the tolerance the candidate gate + SoA resolver must reproduce.
   *
   * MapLibre's point-in-feature test buffers a LINE by half its rendered width
   * and a CIRCLE by its radius. The old hitboxes rendered the shot line at
   * width `hit_area` (reach = hit_area / 2) and the station circle at radius
   * `hit_area` (reach = hit_area). These are also passed to the SoA resolver
   * as the point-query `maxDistance`, so gate and resolver agree exactly.
   */
  private getShotHitReach(): number {
    return this.getShotHitArea() / 2;
  }

  private getStationHitReach(): number {
    return this.getStationHitArea();
  }

  private getStationHoverRadius(): number {
    return this.config?.map?.station?.hover_radius ?? SurveyLayer.DEFAULTS.station.hover_radius;
  }

  private getStationSelectionRadius(): number {
    return this.config?.map?.station?.selection_radius ?? SurveyLayer.DEFAULTS.station.selection_radius;
  }

  private getShotHoverWidth(): number {
    return this.config?.map?.shot?.hover_width ?? SurveyLayer.DEFAULTS.shot.hover_width;
  }

  private getShotSelectionWidth(): number {
    return this.config?.map?.shot?.selection_width ?? SurveyLayer.DEFAULTS.shot.selection_width;
  }

  private initializeEventHandlers() {
    // UNIFIED click handler - gates candidates via the visible layers with
    // station priority. Replaces per-layer click handlers for nearest-to-click.
    this.map.on('click', this.handleMapClick);

    // Boundary filter for the residual VT reload-removed ErrorEvents that
    // MapLibre fires from its own _loadTile catch, upstream of the registry's
    // guards (see handleMapError). Installed once here; removed in destroy().
    this.map.on('error', this.handleMapError);

    // Note: Per-layer click handlers removed - unified handler handles all click logic

    // Process hover update for a given mouse event
    const processHoverUpdate = (e: maplibregl.MapMouseEvent) => {
      // Padded visible-layer query gates the candidate sections (the layers
      // already apply visibility + reference filters); the SoA resolver picks
      // the exact shot/station with the same tolerances the old hitbox had (#mem).
      const { stationSections, shotSections } = this.hitCandidateSections(e.point);

      // Resolve nearest of each type within the hit reach the old hitbox
      // layers gave (station circle radius / shot half line width).
      const stationHitReach = this.getStationHitReach();
      const shotHitReach = this.getShotHitReach();
      const nearestStation = this.resolveStationAt(e.point.x, e.point.y, stationSections, stationHitReach);
      const nearestShot = this.resolveShotAt(e.point.x, e.point.y, shotSections, shotHitReach);

      // Determine what to highlight (stations take priority unless suppressed by tool)
      let newHoveredStationId: string | null = null;
      let newHoveredShotId: string | null = null;

      // Tool-specific hover suppression:
      // - move_station: only stations hover (suppress shot hover)
      // - move_shot: only shots hover (suppress station hover)
      const suppressStationHover = this.activeTool === 'move_shot';
      const suppressShotHover = this.activeTool === 'move_station';

      if (nearestStation && !suppressStationHover) {
        newHoveredStationId = `${nearestStation.ref.section_id}:${nearestStation.ref.station_index}`;
      }
      // Show shot hover when no station is hovered (or station was suppressed) and shots not suppressed.
      if (!newHoveredStationId && nearestShot && !suppressShotHover) {
        newHoveredShotId = `${nearestShot.sectionId}:${nearestShot.shotId}`;
      }

      // Update state only if changed
      const stationChanged = newHoveredStationId !== this.hoveredStationId;
      const shotChanged = newHoveredShotId !== this.hoveredShotId;

      if (stationChanged || shotChanged) {
        if (this.shouldLogInteraction()) {
          console.debug('[HitDetection] Hover update', {
            activeTool: this.activeTool,
            newHoveredStationId,
            newHoveredShotId,
            suppressStationHover,
            suppressShotHover,
            stationHitReach,
            shotHitReach,
            stationCandidateSections: stationSections.size,
            shotCandidateSections: shotSections.size,
          });
        }
        this.hoveredStationId = newHoveredStationId;
        this.hoveredShotId = newHoveredShotId;

        this.refreshHoverCursor();

        // Update MapLibre visual state
        this.updateVisualState();

        // Notify React callbacks (deferred)
        queueMicrotask(() => {
          this.callbacks.onFeatureHover?.(Boolean(newHoveredStationId || newHoveredShotId));

          if (stationChanged) {
            if (newHoveredStationId) {
              const [secId, stIdx] = newHoveredStationId.split(':').map(Number);
              this.callbacks.onStationHover?.({ section_id: secId!, station_index: stIdx! });
            } else {
              this.callbacks.onStationHover?.(null);
            }
          }

          if (shotChanged) {
            if (newHoveredShotId) {
              const [secId, shId] = newHoveredShotId.split(':').map(Number);
              this.callbacks.onShotHover?.({ sectionId: secId!, shotId: shId! });
            } else {
              this.callbacks.onShotHover?.(null);
            }
          }
        });
      }
    };

    // Mousemove only queues the event: the latest one is hit-tested once per
    // animation frame (see HoverFrame), so slow hit-tests never build a backlog.
    const hoverFrame = new HoverFrame(processHoverUpdate);
    this.hoverFrame = hoverFrame;
    this.boundMouseMove = (e: maplibregl.MapMouseEvent) => {
      if (isNonInteractiveTool(this.activeTool)) return;
      if (this.drag.dragPreviewActive) return; // Suppress hover during active drag
      hoverFrame.push(e);
    };

    // Clear all hover when cursor leaves the map canvas
    this.boundMapLeave = () => {
      // First take back a move queued before the leave, which would otherwise
      // hit-test next frame and re-apply the hover with the pointer gone (#576
      // D39(h)). Unconditional: the queued move may be the FIRST hover.
      hoverFrame.cancel();
      if (this.hoveredStationId || this.hoveredShotId) {
        if (this.shouldLogInteraction()) {
          console.debug('[HitDetection] Hover cleared on map leave', {
            previousHoveredStationId: this.hoveredStationId,
            previousHoveredShotId: this.hoveredShotId,
          });
        }
        this.hoveredStationId = null;
        this.hoveredShotId = null;
        // Tell the cursor owner nothing is hovered, so it falls back to the
        // tool's resting cursor (#316: a bare `''` written here once left an arrow
        // that nothing restored, since re-entry only re-derives when the hovered
        // id CHANGES and this handler already nulled it).
        this.refreshHoverCursor();
        // Update visual state immediately
        this.updateVisualState();
        // Defer React callbacks
        queueMicrotask(() => {
          this.callbacks.onFeatureHover?.(false);
          this.callbacks.onStationHover?.(null);
          this.callbacks.onShotHover?.(null);
        });
      }
    };

    this.map.on('mousemove', this.boundMouseMove);
    // `mouseout`, not `mouseleave`: the canvas has no children, so both fire on
    // the same moves. But React enters a panel row (onMouseEnter) from this
    // mouseout as it bubbles to the root, and mouseleave comes after that:
    // cleared there, the hover the row had just put in the store went too (#576
    // W39-NF4). This target listener and its microtask run before the root's
    // listener, so the row's hover lands last.
    this.map.getCanvas().addEventListener('mouseout', this.boundMapLeave);

    // UNIFIED context menu handler - gates candidates via the visible layers
    this.map.on('contextmenu', this.handleContextMenu);

    // Touch long-press → context menu (raw touch listeners on canvas)
    const canvas = this.map.getCanvas();
    this.detachLongPressTouch = attachLongPressTouch(canvas, {
      onLongPress: (pressPoint) => {
        // Hit-test at touch position
        const rect = canvas.getBoundingClientRect();
        const point: [number, number] = [pressPoint.clientX - rect.left, pressPoint.clientY - rect.top];
        const fakeEvent = {
          point: { x: point[0], y: point[1] },
          lngLat: this.map.unproject(point),
          originalEvent: new MouseEvent('contextmenu', {
            clientX: pressPoint.clientX,
            clientY: pressPoint.clientY,
          }),
        } as maplibregl.MapMouseEvent;
        this.handleContextMenu(fakeEvent);
        // handleContextMenu dispatches a menu for every target, including the
        // empty map - but only through `callbacks.onContextMenu`, which is
        // optional. Swallow the tap exactly when a menu could actually open
        // (the helper's contract); with no consumer wired, the tap stays live.
        return this.callbacks.onContextMenu !== undefined;
      },
      isEnabled: () => !this.modalInteractionActive(),
    });

    // Track user-initiated map navigation (zoom & pan) for telemetry
    this.map.on('zoomend', this.handleZoomEndTelemetry);
    this.map.on('moveend', this.handleMoveEndTelemetry);
  }

  /**
   * True while a mutating pointer gesture owns the touch and the long-press has
   * to stay out of it: placement mode, which MapCanvas confirms on `pointerdown`,
   * and a move ghost in flight, which it commits on `pointerup`. Those are
   * POINTER events, not the compatibility mouse events a fired long-press
   * swallows at `touchend`, so they run either way - ungated, one hold would both
   * mutate the survey and open a menu over the result.
   *
   * Both signals are read live rather than pushed in: the ghost is this layer's
   * own (`beginStationDragPreview` / `beginDragPreview` -> `endDragPreview`), and
   * placement mode is uiStore state. Note pointer events precede touch events, so
   * a placement CONFIRMED by this very touch has already cleared the store by the
   * time the press is armed; that is what {@link notePlacementConfirmed} bridges,
   * while the store read covers a hold that starts while placement is still armed
   * (a confirm that bailed, or the next section of a multi-section placement).
   */
  private modalInteractionActive(): boolean {
    return (
      this.drag.dragPreviewActive ||
      useUIStore.getState().placementMode !== null ||
      performance.now() - this.lastPlacementConfirmAt < SurveyLayer.PLACEMENT_CONFIRM_GRACE_MS
    );
  }

  /**
   * Called by MapCanvas when a placement confirms (on `pointerdown`). The store
   * is already cleared by the time this touch's `touchstart` arms the long-press,
   * so without this stamp a hold that just placed a station would ALSO open the
   * context menu over it. The grace window only needs to outlive the
   * pointerdown->touchstart gap (microseconds plus main-thread jitter); it is kept
   * well under the 500ms hold so it can never eat a deliberate later long-press.
   */
  private static readonly PLACEMENT_CONFIRM_GRACE_MS = 200;
  notePlacementConfirmed(): void {
    this.lastPlacementConfirmAt = performance.now();
  }

  /**
   * Re-scope the custom label layer after the programmatic fit-to-bounds settles
   * (MapCanvas calls this on project load). The custom label controller owns its
   * own zoom hysteresis + viewport rebuild on camera settle; this forces a
   * fresh-geometry re-sync so the labels reflect the fitted camera immediately.
   */
  resetLabelZoomHysteresis(): void {
    if (!hasLiveStyle(this.map)) return;
    this.resyncCustomRenderFromStore();
  }

  /** Track user-initiated zoom / pan for telemetry (see surveyLayer/navTelemetry.ts).
   *  Per-instance arrows, so destroy() unregisters exactly this layer's listeners. */
  private handleZoomEndTelemetry = (e: maplibregl.MapLibreEvent): void => trackZoomEndNav(e);
  private handleMoveEndTelemetry = (e: maplibregl.MapLibreEvent): void => trackMoveEndNav(e);

  /**
   * UNIFIED click handler - gates candidates via the visible layers and resolves priority.
   * Stations take priority over shots unless the active tool suppresses one type
   * (move_shot suppresses stations, move_station suppresses shots).
   */
  // Boundary filter for the residual VT reload-removed noise MapLibre fires from
  // its own _loadTile catch, upstream of the registry guards (rationale on
  // shouldSuppressMapError). Suppress exactly that message for this survey's
  // stalx:// sources; re-log every other error, since MapLibre only console.errors
  // an ErrorEvent when it has NO listener (verified vs maplibre-gl 5.16
  // Evented.fire), so registering this listener would otherwise silence the map's
  // default logging for ALL errors.
  private handleMapError = (evt: MapErrorEventLike): void => {
    if (shouldSuppressMapError(evt, AUX_IDS.sourceId)) return;
    console.error(evt?.error ?? evt);
  };

  private handleMapClick = (e: MapMouseEvent): void => {
    // Ctrl+click in move tools is for panning, not interacting with features
    if (e.originalEvent.ctrlKey &&
        (this.activeTool === 'move_station' || this.activeTool === 'move_shot' || this.activeTool === 'move_segment')) {
      return;
    }

    // Tool-specific suppression (consistent with hover in processHoverUpdate)
    const suppressStationClick = this.activeTool === 'move_shot';
    const suppressShotClick = this.activeTool === 'move_station';

    // Padded visible-layer query gates candidate sections; SoA resolver picks
    // the exact item (#mem) - same targets/tolerances as the per-feature path.
    const { stationSections, shotSections } = this.hitCandidateSections(e.point);

    const stationHitReach = this.getStationHitReach();
    const shotHitReach = this.getShotHitReach();

    if (this.semanticTargetPreference.handleClick(e, stationHitReach, shotHitReach, suppressStationClick, suppressShotClick)) return;

    // Stations take priority - resolve nearest station first
    const nearestStation = !suppressStationClick
      ? this.resolveStationAt(e.point.x, e.point.y, stationSections, stationHitReach)
      : null;
    if (nearestStation) {
      const ref: StationRef = nearestStation.ref;
      // Debug logging - only on click, NOT hover (prevents console spam)
      if (this.config?.debug?.map?.log_hit_detection) {
        console.log('[HitDetection] Station click', {
          layer: AUX_IDS.stationsLayerId,
          featureId: `${ref.section_id}:${ref.station_index}`,
          coordinates: e.lngLat,
          candidateSections: stationSections.size,
        });
      }
      this.callbacks.onStationClick?.(ref, e);
      return;
    }

    // Then shots - resolve nearest shot
    const nearestShot = !suppressShotClick
      ? this.resolveShotAt(e.point.x, e.point.y, shotSections, shotHitReach)
      : null;
    if (nearestShot) {
      // Debug logging - only on click, NOT hover (prevents console spam)
      if (this.config?.debug?.map?.log_hit_detection) {
        console.log('[HitDetection] Shot click', {
          layer: 'survey-shots',
          featureId: `${nearestShot.sectionId}:${nearestShot.shotId}`,
          coordinates: e.lngLat,
          candidateSections: shotSections.size,
        });
      }
      this.callbacks.onShotClick?.(nearestShot.sectionId, nearestShot.shotId, e);
      return;
    }

    // Empty click - check for hidden features before firing empty callback
    this.handleEmptyClick(e);
  };

  /**
   * UNIFIED context menu handler - queries the visible layers and resolves priority.
   * Same pattern as click handler: stations take priority, uses nearest resolution.
   */
  private handleContextMenu = (e: MapMouseEvent): void => {
    // Same candidate-section gating + SoA resolution as the click handler.
    const { stationSections, shotSections } = this.hitCandidateSections(e.point);

    const stationHitReach = this.getStationHitReach();
    const shotHitReach = this.getShotHitReach();

    if (this.semanticTargetPreference.handleContextMenu(e, stationHitReach, shotHitReach)) return;

    // Stations take priority
    const nearestStation = this.resolveStationAt(e.point.x, e.point.y, stationSections, stationHitReach);
    if (nearestStation) {
      const ref: StationRef = nearestStation.ref;
      // Debug logging - only on contextmenu, NOT hover (prevents console spam)
      if (this.config?.debug?.map?.log_hit_detection) {
        console.log('[HitDetection] Station context menu', {
          layer: AUX_IDS.stationsLayerId,
          featureId: `${ref.section_id}:${ref.station_index}`,
          coordinates: e.lngLat,
          candidateSections: stationSections.size,
        });
      }
      this.callbacks.onContextMenu?.({ type: 'station', ref }, e);
      return;
    }

    // Then shots
    const nearestShot = this.resolveShotAt(e.point.x, e.point.y, shotSections, shotHitReach);
    if (nearestShot) {
      // Debug logging - only on contextmenu, NOT hover (prevents console spam)
      if (this.config?.debug?.map?.log_hit_detection) {
        console.log('[HitDetection] Shot context menu', {
          layer: 'survey-shots',
          featureId: `${nearestShot.sectionId}:${nearestShot.shotId}`,
          coordinates: e.lngLat,
          candidateSections: shotSections.size,
        });
      }
      const { sectionId, shotId, t: alongT } = nearestShot;
      this.callbacks.onContextMenu?.({ type: 'shot', sectionId, shotId, alongT }, e);
      return;
    }

    // Empty context menu
    this.callbacks.onContextMenu?.(null, e);
  };

  /** Candidate sections under a point / box, gated by the user visibility toggles
   *  (see SurveyHitTester.hitCandidateSections). */
  private hitCandidateSections(
    geometry: maplibregl.PointLike | [maplibregl.PointLike, maplibregl.PointLike],
  ): { stationSections: Set<number>; shotSections: Set<number> } {
    return this.hits.hitCandidateSections(geometry);
  }

  /** Nearest visible station among candidate sections, within maxDistance px. */
  private resolveStationAt(
    x: number,
    y: number,
    sections: Set<number>,
    maxDistance: number,
  ): ReturnType<SurveyGeometryIndex['nearestStation']> {
    return this.hits.resolveStationAt(x, y, sections, maxDistance);
  }

  /** Nearest visible shot among candidate sections, within maxDistance px. */
  private resolveShotAt(
    x: number,
    y: number,
    sections: Set<number>,
    maxDistance: number,
  ): ReturnType<SurveyGeometryIndex['nearestShot']> {
    return this.hits.resolveShotAt(x, y, sections, maxDistance);
  }

  /** Prefer one exact co-located feature for the next development-harness click. */
  setSemanticTargetPreference(target: SemanticMapTarget | null): void { this.semanticTargetPreference.set(target); }

  /**
   * Handle empty click - check for hidden features before firing empty callback.
   * This provides toast feedback when clicking near hidden stations/shots.
   */
  private handleEmptyClick(e: MapMouseEvent): void {
    const HIDDEN_CLICK_THRESHOLD_PX = 15;

    // Probe hidden features through the SoA geometry index, not
    // querySourceFeatures: when the custom geometry layer is active the stock
    // `-stations` / `-shots` VT sources are dropped and serve empty tiles, so a
    // source query returns nothing and the "clicked near a hidden feature" toast
    // is silently lost. `geoIndex` holds every station/shot regardless of the
    // display toggle. Stations take priority over shots (matches prior behavior).
    const allSections = this.geoIndex ? new Set(this.cachedSections.map((s) => s.id)) : null;

    if (allSections && !this.stationsVisible && this.callbacks.onHiddenStationClick) {
      if (this.resolveStationAt(e.point.x, e.point.y, allSections, HIDDEN_CLICK_THRESHOLD_PX)) {
        this.callbacks.onHiddenStationClick();
        return;
      }
    }

    if (allSections && !this.shotsVisible && this.callbacks.onHiddenShotClick) {
      if (this.resolveShotAt(e.point.x, e.point.y, allSections, HIDDEN_CLICK_THRESHOLD_PX)) {
        this.callbacks.onHiddenShotClick();
        return;
      }
    }

    // No hidden features nearby, fire empty click
    this.callbacks.onEmptyClick?.(e);
  }

  // Convert "sectionId:index" string ID to the numeric featureId the renderer keys on
  private parseFeatureId(id: string | null): number | null {
    if (!id) return null;
    const [sectionId, index] = id.split(':').map(Number);
    if (isNaN(sectionId!) || isNaN(index!)) return null;
    return sectionId! * FEATURE_ID_STRIDE + index!;
  }

  /** Whether whatever is hovered RIGHT NOW may show the pointer: suppressed for
   *  anchored or immovable features. Derived purely from current state so it can be
   *  re-run when a dim set changes, not only when the hovered id changes - the
   *  anchored and immovable sets arrive asynchronously, and without a recompute the
   *  cursor disagrees with the highlight until the pointer leaves the feature (#301).
   *  The verdict goes to the map's cursor owner (mapCursor.ts, #576 D3), which
   *  weighs it against the tool, the Ctrl pan and placement; this class no longer
   *  writes the cursor itself. */
  private refreshHoverCursor(): void {
    let showPointer = Boolean(this.hoveredStationId || this.hoveredShotId);
    let suppressReason: string | null = null;
    const hoveredId = this.hoveredStationId || this.hoveredShotId;
    if (showPointer && this.activeTool === 'move_segment' && hoveredId) {
      // Check if hovered feature belongs to an anchored section
      if (this.isSectionAnchored(sectionIdFromKey(hoveredId))) {
        showPointer = false;
        suppressReason = 'anchored_section';
      }
    }
    // Suppress cursor for immovable features in Move Station / Move Shot tools
    if (showPointer && (this.activeTool === 'move_station' || this.activeTool === 'move_shot') && hoveredId) {
      const [secStr, idxStr] = hoveredId.split(':');
      const featureId = Number(secStr) * FEATURE_ID_STRIDE + Number(idxStr);
      const immovable = this.hoveredStationId
        ? this.isStationImmovable(featureId)
        : this.isShotImmovable(featureId);
      if (immovable) {
        showPointer = false;
        suppressReason = this.hoveredStationId ? 'immovable_station' : 'immovable_shot';
      }
    }
    mapCursorFor(this.map).set({ hoverPointer: showPointer });
    if (this.shouldLogInteraction() && suppressReason) {
      console.debug('[HitDetection] Hover suppressed', {
        activeTool: this.activeTool,
        suppressReason,
        hoveredStationId: this.hoveredStationId,
        hoveredShotId: this.hoveredShotId,
      });
    }
  }

  // Check if a section is anchored (suppresses hover/interaction, dims its geometry).
  // Takes the SECTION id, never a featureId: a shot's featureId is not decodable back
  // to one (shot.id is itself section-encoded, so floor(fid / STRIDE) yields 2*sectionId
  // for shots - #301). Every caller already holds the real section id.
  private isSectionAnchored(sectionId: number): boolean {
    return this.anchoredSectionIds.size > 0 && this.anchoredSectionIds.has(sectionId);
  }

  // Check if a station feature is individually immovable (zero-datum or transitively connected)
  private isStationImmovable(featureId: number): boolean {
    return this.immovableStationFeatureIds.has(featureId);
  }

  // Check if a shot feature is individually immovable (endpoint is zero-datum or transitively connected)
  private isShotImmovable(featureId: number): boolean {
    return this.immovableShotFeatureIds.has(featureId);
  }

  private shouldLogInteraction(): boolean {
    return this.config?.debug?.map?.log_hit_detection ?? false;
  }

  // Update visual state. Section-level hover/selection highlights are drawn by the
  // custom renderer (setSectionHover / setSectionSelect / setHighlight, wired from
  // the setHoveredSection / setSelectedSection / setHoveredSegment setters);
  // individual hover/selection/snap render via the interact overlay (#mem).
  private updateVisualState() {
    this.refreshInteractOverlay();
  }

  /**
   * Rebuild the interact overlay source from the current individual
   * interaction state (hovered/selected shot + station, snapped/highlighted
   * stations). A handful of features per update; geometry and colors come
   * from the SoA resolver + the same helpers the bulk build used, so the
   * overlay renders pixel-identical to what per-feature feature-state
   * rendered. Skipped when the composed state key is unchanged.
   */
  private refreshInteractOverlay(): void {
    if (!hasLiveStyle(this.map)) return;
    const source = this.map.getSource(AUX_IDS.interactSourceId);
    if (!source || source.type !== 'geojson') return;

    const key = [
      this.hoveredShotId,
      this.selectedShotId,
      this.hoveredStationId,
      this.selectedStationId,
      this.snappedStationId,
      this.highlightedStationIds.join(','),
    ].join('§');
    // During a drag the feature IDs are unchanged but their coordinates move, so
    // the id-based cache would wrongly short-circuit - bypass it while following.
    if (this.drag.dragOverlayOffset === null && key === this.lastInteractKey) return;
    this.lastInteractKey = key;

    const features: GeoJSON.Feature[] = [];

    // While the custom ghost is active, the grabbed MOVING feature's highlight is
    // drawn IN the ghost (shares its offset uniform, no lag), so suppress the
    // per-frame overlay copy for moving features; non-moving overlay state stays.
    const suppress = this.dragGhost.isCustom();
    const keepShot = (id: string | null): string | null =>
      suppress && id && this.drag.dragMovingShots.has(id) ? null : id;
    const keepStation = (id: string | null): string | null =>
      suppress && id && this.drag.dragMovingStationKeys.has(id) ? null : id;

    // Shots: hover wins over selection on the same shot (matches the old
    // paint case order: hover width > selected width).
    const hoveredShot = keepShot(this.hoveredShotId);
    const selectedShot = keepShot(this.selectedShotId !== this.hoveredShotId ? this.selectedShotId : null);
    if (hoveredShot) {
      const fid = this.parseFeatureId(hoveredShot);
      // Old behavior: no hover feedback for anchored/immovable shots.
      if (fid === null || (!this.isSectionAnchored(sectionIdFromKey(hoveredShot)) && !this.isShotImmovable(fid))) {
        const f = this.shotOverlayFeature(hoveredShot, this.getShotHoverWidth());
        if (f) features.push(f);
      }
    }
    if (selectedShot) {
      const f = this.shotOverlayFeature(selectedShot, this.getShotSelectionWidth());
      if (f) features.push(f);
    }

    // Stations: hover wins over selection on the same station.
    const hoveredStation = keepStation(this.hoveredStationId);
    const selectedStation =
      keepStation(this.selectedStationId !== this.hoveredStationId ? this.selectedStationId : null);
    if (hoveredStation) {
      const fid = this.parseFeatureId(hoveredStation);
      if (fid === null || (!this.isSectionAnchored(sectionIdFromKey(hoveredStation)) && !this.isStationImmovable(fid))) {
        const f = this.stationOverlayFeature(hoveredStation, 'hover');
        if (f) features.push(f);
      }
    }
    if (selectedStation) {
      const f = this.stationOverlayFeature(selectedStation, 'selected');
      if (f) features.push(f);
    }

    // Snap indicator + highlighted stations (both use the snap ring layer).
    const snapIds = new Set<string>(this.highlightedStationIds);
    if (this.snappedStationId) snapIds.add(this.snappedStationId);
    for (const id of snapIds) {
      const [secStr, idxStr] = id.split(':');
      const coords = this.geoIndex?.stationCoords({
        section_id: Number(secStr),
        station_index: Number(idxStr),
      });
      if (coords) {
        features.push({
          type: 'Feature',
          geometry: { type: 'Point', coordinates: coords },
          properties: { kind: 'snap' },
        });
      }
    }

    this.lastOverlayDigest = overlayPaintDigest(features);
    (source as maplibregl.GeoJSONSource).setData({ type: 'FeatureCollection', features });
  }

  /** The color context matching the last build (see features.ts helpers). */
  private displayColorContext(): DisplayColorContext {
    return {
      showDepthGradient: this.showDepthGradient,
      depthRange: this.depthRange,
      themeColors: this.themeColors,
      showIssueSeverity: this.showIssueSeverity && !!this.cachedIssueSeverity,
      issueSeverity: this.cachedIssueSeverity,
      planIdleTint: this.overlays.isPlanIdleActive(),
    };
  }

  /** deps for the extracted interact-overlay feature builders (#157 split). */
  private interactOverlayDeps(): InteractOverlayDeps {
    return {
      geoIndex: this.geoIndex,
      cachedSurveyData: this.cachedSurveyData,
      visibleKinds: this.visibleKinds(),
      parseFeatureId: (id) => this.parseFeatureId(id),
      measuredPathIds: this.measuredPathIds,
      measuredPathStationIds: this.measuredPathStationIds,
      measureTintColor: this.measureTintColor,
      isSectionAnchored: (sectionId) => this.isSectionAnchored(sectionId),
      isPlanIdleActive: this.overlays.isPlanIdleActive(),
      displayColorContext: this.displayColorContext(),
      themeColors: this.themeColors,
      distanceMode: this.overlays.distanceMode,
      distanceState: this.overlays.distanceState,
      gasReachMode: this.overlays.gasReachMode,
      gasReachState: this.overlays.gasReachState,
      dragOverlayOffset: this.drag.dragOverlayOffset,
      dragMovingShots: this.drag.dragMovingShots,
      dragMovingStationKeys: this.drag.dragMovingStationKeys,
      dragProjector: () => this.dragProjector(),
    };
  }

  /** See interactOverlayFeatures.ts: one interact-overlay feature for a shot. */
  private shotOverlayFeature(idStr: string, width: number): GeoJSON.Feature | null {
    return shotOverlayFeature(this.interactOverlayDeps(), idStr, width);
  }

  /** See interactOverlayFeatures.ts: one interact-overlay feature for a station. */
  private stationOverlayFeature(idStr: string, style: 'hover' | 'selected'): GeoJSON.Feature | null {
    const radius = style === 'hover' ? this.getStationHoverRadius() : this.getStationSelectionRadius();
    return stationOverlayFeature(this.interactOverlayDeps(), idStr, style, radius);
  }

  /**
   * Apply the distance-gradient overlay (#34). `enabled` swaps the shots/stations
   * color paint to the distance ramp (or restores section/depth color); `state`
   * carries per-feature `dist` (feature-id → distance, -1 = unreachable).
   * Re-rooting just calls this again - no feature rebuild.
   */
  setDistanceGradient(enabled: boolean, state: DistanceFeatureState | null): void {
    // Unchanged (the off -> off every survey-data change sends): nothing to redraw.
    if (this.overlays.distanceMode === enabled && this.overlays.distanceState === state) return;
    this.overlays.distanceMode = enabled;
    this.overlays.distanceState = state;
    if (!this.map || !hasLiveStyle(this.map)) return;
    // Interact overlay follows the mode swap; the custom renderer (no feature-state)
    // rebuilds the survey geometry with the new overlay colors.
    this.applyOverlayPaint();
    this.resyncOverlayColors();
  }

  /**
   * Apply the gas reachable-frontier overlay (#38). Mirrors
   * {@link setDistanceGradient}: `enabled` swaps the shots/stations color to the
   * gas ramp (gold near the origin → purple at the turn budget, grey =
   * unreachable); `state` carries per-feature `gasReach` (litres-to-reach).
   * Re-running on an input change just calls this again - no feature rebuild.
   */
  setGasReach(enabled: boolean, state: GasReachFeatureState | null): void {
    // Unchanged (the off -> off every survey-data change sends): nothing to redraw.
    if (this.overlays.gasReachMode === enabled && this.overlays.gasReachState === state) return;
    this.overlays.gasReachMode = enabled;
    this.overlays.gasReachState = state;
    if (!this.map || !hasLiveStyle(this.map)) return;
    // Interact overlay follows the mode swap; the custom renderer rebuilds the
    // survey geometry with the new overlay colors.
    this.applyOverlayPaint();
    this.resyncOverlayColors();
    // Beyond-gas-range dimming (#395) follows the same mode/state - toggling
    // Plan mode (or a budget change) applies/clears the gas-dim set.
    this.syncCustomDimming();
  }

  /**
   * Tint the whole survey uniformly in the gas ramp's dim far color (#395
   * revision) while Plan mode is active but no gas origin has been chosen yet -
   * keeps the map coherent with the gas color scheme before there is a field to
   * shade. Mirrors {@link setGasReach}'s wiring exactly (recolor + redim, no
   * feature rebuild); cleared the instant a real field goes live or Plan mode
   * exits (MapCanvas drives both).
   */
  setPlanIdleTint(enabled: boolean): void {
    if (this.overlays.planIdleTint === enabled) return;
    this.overlays.planIdleTint = enabled;
    if (!this.map || !hasLiveStyle(this.map)) return;
    this.applyOverlayPaint();
    this.resyncCustomRenderFromStore();
    this.syncCustomDimming();
  }

  /**
   * Apply the gas route highlight (#233): the picked penetration route drawn over the survey -
   * swum legs at full opacity, jump legs faded (#243). Has its own GeoJSON source (survives
   * `setData()`); the legs are stored so `flushUpdate` re-pushes them after a feature
   * rebuild. `[]` clears it.
   */
  setGasRoute(legs: RouteLeg[], colors: string[] = []): void {
    this.gasRouteLegs = legs.map((leg, i) => ({ ...leg, color: colors[i] ?? '#22d3ee' }));
    applyGasRoute(this.map, this.gasRouteLegs, this.cachedStations, this.gpsOrigin);
  }

  /**
   * Keep the interact overlay's color inputs in step with the active overlay
   * mode (#mem): plain state reads the resolved per-feature `color` property;
   * distance / gas-reach modes read the carried `dist` / `gasReach` value
   * through the SAME ramp expressions the bulk layers use, so the overlay's
   * colors are GPU-identical to what feature-state produced.
   */
  private applyInteractPaint(): void {
    if (!this.map || !hasLiveStyle(this.map)) return;
    const shotColor = this.overlays.interactColorExpression(this.themeColors);
    if (this.map.getLayer(AUX_IDS.interactShotsLayerId)) {
      this.map.setPaintProperty(AUX_IDS.interactShotsLayerId, 'line-color', shotColor);
    }
    if (this.map.getLayer(AUX_IDS.interactStationsLayerId)) {
      this.map.setPaintProperty(AUX_IDS.interactStationsLayerId, 'circle-color', shotColor);
    }
  }

  /** Refresh the interact overlay for the active analytical-overlay mode (the
   *  distance gradient or the gas reachable-frontier). The custom renderer draws
   *  the survey geometry in the overlay colors (via syncCustomRender's colorCtx);
   *  here we only keep the interact overlay's colors + features in step. */
  private applyOverlayPaint(): void {
    if (!this.map) return;
    // The interact overlay's color inputs follow the mode swap; its features also
    // need rebuilding (they carry mode-scoped dist/gasReach).
    this.applyInteractPaint();
    this.lastInteractKey = null;
    this.refreshInteractOverlay();
  }

  /** Tint the measured along-cave-line path's shots AND stations (#26) in `color`,
   *  or clear with empty/null sets. Rendered by the dedicated custom measure-path
   *  WebGL layer (syncCustomMeasurePath); the interact overlay carries the tint
   *  for a hovered/selected measured shot. */
  setMeasuredPath(shotIds: Set<number> | null, stationIds: Set<number> | null, color: string): void {
    this.measuredPathIds = shotIds && shotIds.size > 0 ? shotIds : null;
    this.measuredPathStationIds = stationIds && stationIds.size > 0 ? stationIds : null;
    this.measureTintColor = color;
    if (!this.map || !hasLiveStyle(this.map)) return;
    // Overlay features carry the tint decision - rebuild them.
    this.lastInteractKey = null;
    this.refreshInteractOverlay();
    // Drive the dedicated custom measure-path layer (the sole measured-path tint).
    this.syncCustomMeasurePath();
  }

  /** (Re)build the dedicated custom measure-path layer (#26) from the current
   *  measured sets, or clear it; see surveyLayer/measurePathSync.ts. */
  private syncCustomMeasurePath(): void {
    syncMeasurePath(this.measurePath, this.map, {
      geoIndex: this.geoIndex,
      measuredPathIds: this.measuredPathIds,
      measuredPathStationIds: this.measuredPathStationIds,
      shotsVisible: this.shotsVisible,
      stationsVisible: this.stationsVisible,
      visibleKinds: this.visibleKinds(),
      measureTintColor: this.measureTintColor,
      shotWidth: this.getShotWidth(),
      stationRadius: this.getStationRadius(),
    });
  }

  // Set selected station (called from outside to sync with uiStore)
  setSelectedStation(ref: StationRef | null) {
    const newId = ref ? `${ref.section_id}:${ref.station_index}` : null;
    if (this.shouldLogInteraction() && newId !== this.selectedStationId) {
      console.debug('[Selection] Selected station changed', {
        previous: this.selectedStationId,
        next: newId,
      });
    }
    this.selectedStationId = newId;
    this.updateVisualState();
  }

  // Set selected shot (called from outside to sync with uiStore)
  setSelectedShot(sectionId: SectionId | null, shotId: ShotId | null) {
    const newId = sectionId !== null && shotId !== null ? `${sectionId}:${shotId}` : null;
    if (this.shouldLogInteraction() && newId !== this.selectedShotId) {
      console.debug('[Selection] Selected shot changed', {
        previous: this.selectedShotId,
        next: newId,
      });
    }
    this.selectedShotId = newId;
    this.updateVisualState();
  }

  // Set snapped station (called from outside for snap indicator)
  // Pushes a per-vertex attribute update for an instant GPU-side change (same mechanism as hover).
  setSnappedStation(ref: StationRef | null) {
    const newId = ref ? `${ref.section_id}:${ref.station_index}` : null;
    if (this.snappedStationId === newId) return;
    this.snappedStationId = newId;

    // During a live move drag EVERY interact refresh is funneled through the
    // per-frame renderOverlayFollow pass, so the snap ring and the grabbed
    // highlight update together in the SAME rAF tick. A snap-target change is
    // driven by the same pointer move that schedules that frame, so the ring
    // lands one frame later at most; outside a drag, refresh immediately as
    // before. (The moving ghost follows synchronously via its own translate.)
    if (this.drag.dragPreviewActive && !this.drag.dragCommitPending) return;

    // Snap ring renders from the interact overlay (#mem): pushing one small
    // feature is as instant as the old per-feature feature-state was.
    this.refreshInteractOverlay();
  }

  /** Nearest station to a screen point within a circular `threshold` px, or null
   *  (see SurveyHitTester.queryNearestStation). */
  queryNearestStation(
    screenX: number,
    screenY: number,
    threshold: number
  ): StationRef | null {
    return this.hits.queryNearestStation(screenX, screenY, threshold);
  }

  /** Nearest shot to a screen point plus the fraction `t` along it and the snap
   *  point (#26 ruler snap-to-shot), or null when none is within `threshold` px
   *  (see SurveyHitTester.queryNearestShotPoint). */
  queryNearestShotPoint(
    screenX: number,
    screenY: number,
    threshold: number
  ): { section_id: number; shot_id: number; t: number; lng: number; lat: number } | null {
    return this.hits.queryNearestShotPoint(screenX, screenY, threshold);
  }

  /**
   * Set the hovered section for visual highlighting.
   * Stores the section ID for use by updateVisualState() to apply hover styling
   * to all shots and stations in this section.
   * @param sectionId - The SectionId (number) to hover, or null to clear
   */
  setHoveredSection(sectionId: SectionId | null): void {
    // Convert numeric SectionId to string for consistent comparison
    const stringId = sectionId !== null ? String(sectionId) : null;
    if (this.hoveredSectionId === stringId) return;
    if (this.shouldLogInteraction()) {
      console.debug('[Highlight] Hovered section changed', {
        previous: this.hoveredSectionId,
        next: stringId,
      });
    }
    this.hoveredSectionId = stringId;
    // Custom renderer ignores feature-state; drive its per-vertex highlight flag
    // at hover level (the stock section hover used the same `hover` state).
    this.customRender?.setSectionHover(sectionId !== null ? [sectionId] : []);
    this.updateVisualState();
  }

  /**
   * Set multi-section hover for Move Segment tool.
   * Highlights all shots and stations in the given sections (the connected segment).
   * Takes priority over single-section hover when non-empty.
   * @param sectionIds - Array of section IDs in the connected segment, or empty to clear
   */
  setHoveredSegment(sectionIds: number[]): void {
    const stringIds = sectionIds.map(String);
    // Quick equality check (arrays are small, typically <10 sections)
    if (stringIds.length === this.hoveredSegmentSectionIds.length &&
        stringIds.every((id, i) => id === this.hoveredSegmentSectionIds[i])) return;
    if (this.shouldLogInteraction()) {
      console.debug('[Highlight] Hovered segment changed', {
        previous: this.hoveredSegmentSectionIds,
        next: stringIds,
      });
    }
    this.hoveredSegmentSectionIds = stringIds;
    // Custom renderer ignores feature-state; drive its per-vertex highlight flag
    // so the whole connected segment is emphasized under the custom layer too.
    this.customRender?.setHighlight(sectionIds);
    this.updateVisualState();
  }

  /**
   * Set the hovered shot for visual highlighting (external hover, e.g. from DataTable).
   * This allows external components to trigger shot hover highlighting without mouse interaction.
   * @param sectionId - The SectionId of the shot, or null to clear
   * @param shotId - The ShotId of the shot, or null to clear
   */
  setHoveredShot(sectionId: SectionId | null, shotId: ShotId | null): void {
    // Convert to composite string ID format used internally
    const stringId = sectionId !== null && shotId !== null ? `${sectionId}:${shotId}` : null;
    if (this.hoveredShotId === stringId) return;
    if (this.shouldLogInteraction()) {
      console.debug('[Highlight] Hovered shot changed', {
        previous: this.hoveredShotId,
        next: stringId,
      });
    }
    this.hoveredShotId = stringId;
    this.updateVisualState();
  }

  /**
   * Set the hovered station for visual highlighting (external hover, e.g. from DetailLink).
   * This allows external components to trigger station hover highlighting without mouse interaction.
   * @param ref - The StationRef of the station, or null to clear
   */
  setHoveredStation(ref: StationRef | null): void {
    // Convert to composite string ID format used internally
    const stringId = ref !== null ? `${ref.section_id}:${ref.station_index}` : null;
    if (this.hoveredStationId === stringId) return;
    if (this.shouldLogInteraction()) {
      console.debug('[Highlight] Hovered station changed', {
        previous: this.hoveredStationId,
        next: stringId,
      });
    }
    this.hoveredStationId = stringId;
    this.updateVisualState();
  }

  /**
   * Set the selected section for visual highlighting.
   * Stores the section ID for use by updateVisualState() to apply selected styling
   * to all shots and stations in this section.
   * @param sectionId - The SectionId (number) to select, or null to clear
   */
  setSelectedSection(sectionId: SectionId | null): void {
    // Convert numeric SectionId to string for consistent comparison
    const stringId = sectionId !== null ? String(sectionId) : null;
    if (this.selectedSectionId === stringId) return;
    if (this.shouldLogInteraction()) {
      console.debug('[Selection] Selected section changed', {
        previous: this.selectedSectionId,
        next: stringId,
      });
    }
    this.selectedSectionId = stringId;
    // Custom renderer ignores feature-state; drive its per-vertex highlight flag
    // at selection level (smaller bump than hover, matching stock `selected`).
    this.customRender?.setSectionSelect(sectionId !== null ? [sectionId] : []);
    this.updateVisualState();
  }

  // Depth gradient accessors - return current depth mode state
  getShowDepthGradient(): boolean {
    return this.showDepthGradient;
  }

  getDepthRange(): DepthRange {
    // Return a copy to prevent external mutation
    return { min: this.depthRange.min, max: this.depthRange.max };
  }

  // Control shot data labels visibility (custom label layer uniform).
  setShowShotLabels(show: boolean) {
    this.customLabels?.setShotLabelsVisible(show);
  }

  // Control section name labels visibility (custom label layer uniform).
  setShowSectionNames(show: boolean) {
    this.customLabels?.setSectionNamesVisible(show);
  }

  // Control station index labels visibility (custom label layer uniform).
  setShowStationLabels(show: boolean) {
    this.customLabels?.setStationLabelsVisible(show);
  }

  // Control station comments visibility. Effective visibility folds in the
  // comment-tool override (forces visible); the custom uniform mirrors it.
  setShowStationComments(show: boolean) {
    this.cachedShowStationComments = show;
    this.customLabels?.setStationCommentsVisible(this.commentToolActive || show);
  }

  // Control shot comments visibility (custom uniform mirrors the effective state).
  setShowShotComments(show: boolean) {
    this.cachedShowShotComments = show;
    this.customLabels?.setShotCommentsVisible(this.commentToolActive || show);
  }

  // Control stations visibility (also hides loop closures). The custom renderer
  // carries the real stations visibility; hit-testing is gated by stationsVisible
  // via the SoA geometry index, so hidden stations stay non-interactive.
  setShowStations(visible: boolean): void {
    if (!this.map) return;
    this.stationsVisible = visible;
    // Clear hover state FIRST when hiding to prevent any race conditions.
    if (!visible) {
      this.clearStationHover();
    }
    this.customRender?.setStationsVisible(visible);
    this.applyStockVisibility();
    // Note: stationsHighlightLayerId intentionally NOT hidden - it shows only
    // snapped/origin stations even when the main stations layer is hidden.
    // Drop/restore the measured-path station discs with the stations toggle.
    this.syncCustomMeasurePath();
  }

  // Control shots visibility. The custom renderer carries the real shots
  // visibility; hit-testing is gated by shotsVisible via the SoA geometry index.
  setShowShots(visible: boolean): void {
    if (!this.map) return;
    this.shotsVisible = visible;
    // Clear hover state FIRST when hiding to prevent any race conditions.
    if (!visible) {
      this.clearShotHover();
    }
    this.customRender?.setShotsVisible(visible);
    this.applyStockVisibility();
    // Keep the interact overlay's colors + features in step.
    this.applyOverlayPaint();
    // Drop/restore the measured-path shot lines with the shots toggle.
    this.syncCustomMeasurePath();
  }

  // Control reference shots visibility. The custom renderer + custom label layer
  // hide/show reference legs via a shader uniform; the interact overlay + custom
  // measure-path honor the same flag when resolving features.
  /** Which virtual kinds are currently drawn (#27). Hit-testing, the measure
   *  path and the overlay all resolve against this so they agree with the
   *  shader. */
  private visibleKinds(): VisibleKinds {
    return { jump: this.showJumpsCached, reference: this.showReferenceShotsCached };
  }

  /** Show/hide jump shots, independently of references (#27). */
  setShowJumps(visible: boolean): void {
    if (!this.map) return;
    this.showJumpsCached = visible;
    this.customRender?.setJumpShotsVisible(visible);
    this.customLabels?.setJumpShotsVisible(visible);
    this.syncCustomMeasurePath();
    this.lastInteractKey = null;
    this.refreshInteractOverlay();
  }

  setShowReferenceShots(visible: boolean): void {
    if (!this.map) return;
    this.showReferenceShotsCached = visible;
    this.customRender?.setReferenceShotsVisible(visible);
    this.customLabels?.setReferenceShotsVisible(visible);
    // Measured-path tint drops hidden reference legs (syncCustomMeasurePath guard).
    this.syncCustomMeasurePath();
    // Overlay features honor the reference filter (hidden reference shots /
    // all-reference stations produce no highlight) - rebuild them.
    this.lastInteractKey = null;
    this.refreshInteractOverlay();
  }

  /**
   * Hide/show the source comment label during move_comment pickup (C3). Threads
   * the picked-up comment's identity into the custom label layer, which collapses
   * that one comment's label (LABEL_FLAG.COMMENT_HIDE) so only the cursor-following
   * tag shows - matching the deleted stock hidden-comment filter. `null` clears it.
   */
  setHiddenComment(hidden: { type: 'station' | 'shot'; sectionId: number; id: number } | null): void {
    this.customLabels?.setHiddenComment(hidden);
  }

  /** Force both comment kinds visible + overlap-exempt (comment/move_comment
   *  tool): the custom label uniforms mirror the forced-visible override and the
   *  reveal pass stops culling comments so ALL stay visible/selectable (C1). */
  private _enableCommentToolOverride(): void {
    this.commentToolActive = true;
    this.customLabels?.setStationCommentsVisible(true);
    this.customLabels?.setShotCommentsVisible(true);
    this.customLabels?.setCommentOverlapExempt(true);
  }

  /** Restore comment visibility from cached display settings + re-enable comment
   *  collision (custom uniforms + reveal pass). */
  private _disableCommentToolOverride(): void {
    this.commentToolActive = false;
    this.customLabels?.setStationCommentsVisible(this.cachedShowStationComments);
    this.customLabels?.setShotCommentsVisible(this.cachedShowShotComments);
    this.customLabels?.setCommentOverlapExempt(false);
  }

  // Set highlighted stations by their string IDs (format: "sectionId:stationIndex")
  setHighlightedStations(stationIds: string[]): void {
    if (!this.map) return;
    this.highlightedStationIds = stationIds;
    // Highlights render as snap-ring features from the interact overlay (#mem).
    this.refreshInteractOverlay();
  }

  // Clear station hover state
  clearStationHover(): void {
    if (this.shouldLogInteraction() && this.hoveredStationId) {
      console.debug('[Highlight] Clearing station hover', {
        previous: this.hoveredStationId,
      });
    }
    this.hoveredStationId = null;
    this.callbacks.onStationHover?.(null);
    // Only clear feature hover if no shot is being hovered
    if (!this.hoveredShotId) {
      this.callbacks.onFeatureHover?.(false);
    }
    this.refreshHoverCursor();
    this.updateVisualState();
  }

  // Clear shot hover state
  clearShotHover(): void {
    if (this.shouldLogInteraction() && this.hoveredShotId) {
      console.debug('[Highlight] Clearing shot hover', {
        previous: this.hoveredShotId,
      });
    }
    this.hoveredShotId = null;
    this.callbacks.onShotHover?.(null);
    // Only clear feature hover if no station is being hovered
    if (!this.hoveredStationId) {
      this.callbacks.onFeatureHover?.(false);
    }
    this.refreshHoverCursor();
    this.updateVisualState();
  }

  // Clear snapped station state
  clearSnappedStation(): void {
    this.setSnappedStation(null);
  }

  // Clear all interaction state (hover, selection, snap, highlights)
  clearAllInteractionState(): void {
    this.hoveredStationId = null;
    this.hoveredShotId = null;
    this.hoveredSectionId = null;
    this.hoveredSegmentSectionIds = [];
    this.snappedStationId = null;
    this.selectedStationId = null;
    this.selectedShotId = null;
    this.selectedSectionId = null;
    // Custom renderer: drop every section-emphasis source in one refresh.
    this.customRender?.clearHighlights();
    this.setHighlightedStations([]);
    this.callbacks.onStationHover?.(null);
    this.callbacks.onShotHover?.(null);
    this.callbacks.onFeatureHover?.(false);
    this.refreshHoverCursor();
    this.updateVisualState();
  }

  /**
   * Set anchored section IDs for fading when Move Segment tool is active.
   * Sections in this set get reduced opacity via feature-state.
   */
  setAnchoredSections(sectionIds: ReadonlySet<number>): void {
    // The same set, or empty again: nothing to re-dim. A dim pass is two O(shots)
    // recomputes plus buffer uploads, and every edit outside the move tools used
    // to land here with a fresh empty set (#576 D8).
    if (sameOrBothEmpty(sectionIds, this.anchoredSectionIds)) return;
    const before = this.lastOverlayDigest;
    this.anchoredSectionIds = sectionIds;
    // Update loop closure line opacity - gray out closures where both endpoints are anchored.
    this.updateClosureOpacity();
    // The custom renderer + label layer dim the anchored sections' geometry + labels.
    this.syncCustomDimming();
    // The anchored set also gates the interact overlay (hover/selection highlight +
    // its 0.25 opacity), but it is NOT part of the overlay's id-only cache key, and
    // it lands asynchronously after move_segment activates. Without this the
    // already-drawn highlight on a held-still pointer stays bright over geometry the
    // renderer just dimmed - and stays suppressed after leaving the tool.
    this.applyDimChangeToOverlay(before);
  }

  /** Push the current anchored + immovable + gas-dim (#395) set to the custom-render
   *  layers so they dim the same features the stock paint expressions do
   *  (#customRender). Both geometry AND labels take the same {@link CustomDimState};
   *  the label controller reproduces the stock LABEL dim rules (anchored dims shot/
   *  station/section labels, immovable + gas-dim dim individual shot/station labels;
   *  comments and section-name immovable/gas-dim are left bright, matching stock). */
  private syncCustomDimming(): void {
    const dim = buildCustomDimState(this.anchoredSectionIds, this.immovableStationFeatureIds, this.immovableShotFeatureIds, this.overlays.gasReachMode, this.overlays.gasReachState, this.overlays.isPlanIdleActive());
    this.customRender?.setDimming(dim);
    this.customLabels?.setDimming(dim);
  }

  /** The one combined loop-closure dim write (see applyClosureOpacity, #576 NF-MAP-4). */
  private updateClosureOpacity(): void {
    applyClosureOpacity(this.map, this.anchoredSectionIds, this.immovableStationFeatureIds);
  }

  /**
   * Set individual feature-level immovable state for move_station / move_shot tools.
   * Uses setPaintProperty with data-driven expressions to dim immovable features.
   * Unlike setAnchoredSections (which re-tessellates dim flags), this approach is reliable
   * on initial tool activation because it doesn't depend on feature tiling state.
   */
  setImmovableFeatures(stationIds: ReadonlySet<number>, shotIds: ReadonlySet<number>): void {
    // Same no-op skip as setAnchoredSections (#576 D8).
    if (
      sameOrBothEmpty(stationIds, this.immovableStationFeatureIds) &&
      sameOrBothEmpty(shotIds, this.immovableShotFeatureIds)
    ) return;
    const before = this.lastOverlayDigest;
    this.immovableStationFeatureIds = stationIds;
    this.immovableShotFeatureIds = shotIds;
    // Loop closures dim if either endpoint is immovable (kept GeoJSON overlay).
    this.updateClosureOpacity();
    // The custom renderer + label layer dim the immovable geometry + labels.
    this.syncCustomDimming();
    // Same overlay-cache hazard as setAnchoredSections: these sets gate the
    // hover/selection highlight but are not in the overlay's cache key.
    this.applyDimChangeToOverlay(before);
  }

  /** Re-derive everything a dim-set change feeds, given the {@link overlayPaintDigest}
   *  captured before the change: the interact overlay (not in its own cache key),
   *  the hover cursor (otherwise only recomputed when the hovered id changes), and
   *  - only when the emitted overlay actually changed - a hide across the GeoJSON
   *  re-tile, which lags the synchronous custom-geometry dim by ~250ms and would
   *  otherwise flash the old highlight over newly dimmed geometry. Comparing the
   *  BUILT features, rather than re-deriving which dim rule applies to which id,
   *  keeps this in lockstep with what is actually painted. */
  private applyDimChangeToOverlay(before: string): void {
    this.lastInteractKey = null;
    this.refreshInteractOverlay();
    this.refreshHoverCursor();
    if (this.lastOverlayDigest !== before) this.interactHide.hideUntilRetile();
  }

  /** Build the layer-spec context for the drag-preview layers module: shots +
   *  stations color / size / highlight (see surveyLayer/dragPreviewLayers.ts). */
  private dragPreviewLayerCtx(): DragPreviewLayerCtx {
    return {
      colorExpr: this.overlays.dragPreviewColorExpr(this.themeColors),
      shotWidth: this.getShotSelectionWidth(),
      stationRadius: this.getStationSelectionRadius(),
      selectHighlight: this.themeColors.selectHighlight,
    };
  }

  /** Build the custom-ghost render data (buffers + device-px sizes) for a captured
   *  set: colors resolved at capture time the same way the bulk geometry shows
   *  them (base / severity / depth from the feature's own `color`, or the
   *  analytical overlay ramp), the preview's 0.85 line/fill opacity baked into
   *  alpha. Sizes mirror {@link dragPreviewLayerCtx} (selection width/radius,
   *  2.5px stroke) scaled to device px, matching the custom bulk layer's dpr. */
  private dragGhostData(set: DragPreviewSet): GhostRenderData {
    return buildGhostRenderData(set, {
      dpr: (typeof window !== 'undefined' && window.devicePixelRatio) || 1,
      themeColors: this.themeColors,
      overlay: this.overlays.overlayInput(),
      shotsVisible: this.shotsVisible,
      stationsVisible: this.stationsVisible,
      selShotWidth: this.getShotSelectionWidth(),
      hoverShotWidth: this.getShotHoverWidth(),
      selRadius: this.getStationSelectionRadius(),
      hoverRadius: this.getStationHoverRadius(),
      hoveredShotId: this.hoveredShotId,
      selectedShotId: this.selectedShotId,
      hoveredStationId: this.hoveredStationId,
      selectedStationId: this.selectedStationId,
      movingEndForShot: (id) => this.drag.dragMovingShots.get(id),
      isMovingStation: (id) => this.drag.dragMovingStationKeys.has(id),
      parseFid: (id) => this.parseFeatureId(id),
      isAnchored: (id) => this.isSectionAnchored(sectionIdFromKey(id)),
      isShotImmovable: (fid) => this.isShotImmovable(fid),
      isStationImmovable: (fid) => this.isStationImmovable(fid),
      shotFeature: (id, w) => this.shotOverlayFeature(id, w),
      stationFeature: (id, s) => this.stationOverlayFeature(id, s),
    });
  }

  /** The map-backed projector used for the preview + overlay offset transforms. */
  private dragProjector(): Projector {
    return {
      project: (lngLat) => this.map.project(lngLat),
      unproject: (pt) => this.map.unproject(pt),
    };
  }

  /** Begin the SEGMENT drag preview (see DragPreviewController.beginDragPreview). */
  beginDragPreview(connectedSectionIds: Set<number>): void {
    this.drag.beginDragPreview(connectedSectionIds);
  }

  /** Begin the station / shot drag preview; returns the co-located StationRefs for
   *  MapCanvas to batch-move (see DragPreviewController.beginStationDragPreview). */
  beginStationDragPreview(sectionId: number, stationIndex: number, additionalSeeds?: StationRef[]): StationRef[] {
    return this.drag.beginStationDragPreview(sectionId, stationIndex, additionalSeeds);
  }

  /**
   * Update the station / shot drag preview - one setData over the unified source
   * (stations, shots, labels, closures all move together, no skew).
   */
  updateStationDragPreview(dx: number, dy: number): void {
    this.drag.scheduleDragRender(dx, dy);
  }

  /**
   * Update the segment drag preview - one setData over the unified source. The
   * whole segment moves uniformly, so every captured vertex takes the offset.
   */
  updateDragPreview(dx: number, dy: number): void {
    this.drag.scheduleDragRender(dx, dy);
  }

  /** Commit the drag preview before the backend commit; the moved features stay
   *  hidden until endDragPreview (see DragPreviewController.commitDragPreview). */
  commitDragPreview(): void {
    this.drag.commitDragPreview();
  }

  /** End the drag preview and restore the originals (see DragPreviewController.endDragPreview). */
  endDragPreview(): void {
    this.drag.endDragPreview();
  }

  /**
   * Set ghost section data for merge preview visualization.
   * Pass null to clear the ghost section.
   */
  setGhostSection(data: GhostSectionData | null) {
    this.ghostSection = data;
    // No style while the WebGL context is lost; the restore replays it.
    if (!hasLiveStyle(this.map)) return;
    applyGhostSection(this.map, data, this.gpsOrigin);
  }

  /**
   * The stock layers that follow the user's Stations / Shots toggles: loop
   * closures and the interact overlay's station layer hide with the stations, the
   * interact overlay's shot layers with the shots - a hover/selection highlight
   * must not surface a hidden feature (#mem). Skipped while the WebGL context is
   * lost (no style) and while a rebuilt style loads (it refuses the writes);
   * replayAfterContextRestore re-applies it (#576 NF-MAP-5, RV18-MAP-1).
   */
  private applyStockVisibility(): void {
    if (!canWriteStyle(this.map)) return;
    const stations = this.stationsVisible ? 'visible' : 'none';
    const shots = this.shotsVisible ? 'visible' : 'none';
    this.map.setLayoutProperty(AUX_IDS.loopClosuresLayerId, 'visibility', stations);
    this.interactHide.setLayerVisibility(AUX_IDS.interactStationsLayerId, stations);
    this.interactHide.setLayerVisibility(AUX_IDS.interactShotsLayerId, shots);
    this.interactHide.setLayerVisibility(AUX_IDS.interactShotsMeasuredLayerId, shots);
  }

  /**
   * Public update entry point. Captures the args and schedules a single
   * synchronous flush at the next microtask boundary, collapsing any
   * additional calls that arrive in the same tick.
   *
   * Callers that *need* the data on the source synchronously (e.g. drag
   * commit using onNextUpdate) get it because microtasks drain before
   * paint and before any rAF that the drag handler uses to finalize.
   */
  update(
    sections: Section[],
    stations: Station[],
    loopClosures?: LoopClosure[],
    gpsOrigin?: GpsOrigin | null,
    showDepthGradient: boolean = false,
    depthRange: DepthRange = { min: 0, max: 50 },
    precision?: DecimalPrecision,
    showIssueSeverity: boolean = false,
    warnings: ProjectWarning[] = []
  ) {
    // Overwrite the pending args - last-call-wins is correct because the
    // builder is deterministic in its inputs; coalescing equivalent calls
    // is the whole point.
    this.pendingUpdateArgs = {
      sections,
      stations,
      loopClosures,
      gpsOrigin: gpsOrigin ?? null,
      showDepthGradient,
      depthRange,
      precision,
      showIssueSeverity,
      warnings,
    };
    this.coalescedUpdateCallsSinceLastFlush += 1;

    // Drag-commit short-circuit: callers that registered onNextUpdate
    // (e.g. MapCanvas.endDragPreview) expect the setData and the
    // callback to land in the same frame - they're restoring filters
    // that hide the dragged preview while the real geometry catches
    // up.  The default async-worker path delays the callback by the
    // worker round-trip (~100 ms on a 14 k-section project), leaving
    // the user looking at the preview hanging on top of stale features.
    // When a one-shot callback is pending, flush synchronously instead.
    if (this.onNextUpdateCallback) {
      this.updateScheduled = false;
      const args = this.pendingUpdateArgs;
      this.pendingUpdateArgs = null;
      this.flushUpdateSync(
        args.sections,
        args.stations,
        args.loopClosures,
        args.gpsOrigin,
        args.showDepthGradient,
        args.depthRange,
        args.precision,
        args.showIssueSeverity,
        args.warnings,
      );
      return;
    }

    if (this.updateScheduled) return;
    this.updateScheduled = true;
    queueMicrotask(() => {
      this.updateScheduled = false;
      const args = this.pendingUpdateArgs;
      this.pendingUpdateArgs = null;
      if (!args) return;
      // The custom renderer builds synchronously, so the coalesced flush is a
      // direct synchronous flushUpdate (no worker round-trip).
      this.flushUpdate(
        args.sections,
        args.stations,
        args.loopClosures,
        args.gpsOrigin,
        args.showDepthGradient,
        args.depthRange,
        args.precision,
        args.showIssueSeverity,
        args.warnings,
      );
    });
  }

  /**
   * Prototype hook (#customRender): mirror the just-built bulk geometry into the
   * flag-gated custom WebGL layer. No-op unless the gate is on. Reads the caches
   * the flush already populated (surveyData / severity / modes / theme / origin),
   * so it adds nothing to the hot path when the flag is off.
   */
  private syncCustomRender(surveyData: SurveyData): void {
    if (!this.customRender) return;
    const colorCtx: DisplayColorContext = {
      showDepthGradient: this.showDepthGradient,
      depthRange: this.depthRange,
      themeColors: this.themeColors,
      showIssueSeverity: this.showIssueSeverity,
      issueSeverity: this.cachedIssueSeverity,
      // Analytical overlay ramp colors (distance #34 / gas-reach #38): the custom
      // layer has no feature-state, so it resolves them per feature at build time.
      overlay: this.overlays.overlayInput(),
      // Move-comment palette tint (#52): custom features carry no commentColor
      // property, so the resolvers recompute it (only while move_comment is active).
      commentTint: this.activeTool === 'move_comment',
      // Plan-mode idle tint (#395 revision): paints every feature the gas ramp's
      // dim far color while there is no origin yet.
      planIdleTint: this.overlays.isPlanIdleActive(),
    };
    this.customRender.sync({
      surveyData,
      gpsOrigin: this.gpsOrigin,
      colorCtx,
      shotWidthPx: this.getShotWidth(),
      stationRadiusPx: this.getStationRadius(),
      stationStrokePx: 1,
      // Configured highlight bumps (C2): the custom survey shader derives the
      // hover / selection width + radius from these instead of baking 2x/1.5x.
      shotHoverWidthPx: this.getShotHoverWidth(),
      shotSelectionWidthPx: this.getShotSelectionWidth(),
      stationHoverRadiusPx: this.getStationHoverRadius(),
      stationSelectionRadiusPx: this.getStationSelectionRadius(),
    });
    // Custom LABEL layer (#customRender labels). Shares surveyData + theme; its
    // own composed gate no-ops when off. Builds all five kinds, viewport-scoped
    // with collision; a synchronous rebuild here keeps the edit path immediate.
    this.customLabels?.sync({
      surveyData,
      gpsOrigin: this.gpsOrigin,
      themeColors: this.themeColors,
      showDepthGradient: this.showDepthGradient,
      depthRange: this.depthRange,
      precision: this.cachedPrecision,
      commentTint: this.activeTool === 'move_comment',
    });
  }

  /**
   * The custom-render rebuild behind an analytical-overlay change (#576 NF-W13FE-1).
   * A coalesced update() flush still queued rebuilds from the store with the overlay
   * state as it is when it runs, so an overlay set in the same commit as a survey-data
   * change (useAnalyticalOverlays' passive effects run before update()'s microtask)
   * leaves the rebuild to it: one rebuild per change instead of one per setter.
   */
  private resyncOverlayColors(): void {
    if (this.updateScheduled) return;
    this.resyncCustomRenderFromStore();
  }

  /**
   * Re-sync the custom-render prototype from CURRENT store geometry
   * (#customRender). Backs the runtime `__stalagmightSyncCustomRender` hook: the
   * controller's own snapshot is a previous project's geometry after a switch,
   * so read fresh geometry here (measured fallback, like every flush) rather
   * than replaying the stale snapshot. Falls back to the last flush's cached
   * geometry only when the store has no active geometry (e.g. mid-close).
   */
  private resyncCustomRenderFromStore(): void {
    if (!this.customRender || !hasLiveStyle(this.map)) return;
    const surveyData =
      selectActiveSurveyData(useProjectStore.getState(), useUIStore.getState().geometryMode) ??
      this.cachedSurveyData;
    if (surveyData) this.syncCustomRender(surveyData);
  }

  /**
   * Synchronous body of update(). Use this when you genuinely need the
   * setData calls to land in the current stack frame (drag-commit path);
   * normal callers go through the coalescing public `update()` above.
   */
  flushUpdateSync(
    sections: Section[],
    stations: Station[],
    loopClosures?: LoopClosure[],
    gpsOrigin?: GpsOrigin | null,
    showDepthGradient: boolean = false,
    depthRange: DepthRange = { min: 0, max: 50 },
    precision?: DecimalPrecision,
    showIssueSeverity: boolean = false,
    warnings: ProjectWarning[] = [],
  ): void {
    // Cancel any pending coalesced flush so we don't re-run with stale args.
    this.updateScheduled = false;
    this.pendingUpdateArgs = null;
    this.flushUpdate(
      sections,
      stations,
      loopClosures,
      gpsOrigin ?? null,
      showDepthGradient,
      depthRange,
      precision,
      showIssueSeverity,
      warnings,
    );
  }

  private flushUpdate(
    sections: Section[],
    stations: Station[],
    loopClosures: LoopClosure[] | undefined,
    gpsOrigin: GpsOrigin | null,
    showDepthGradient: boolean,
    depthRange: DepthRange,
    precision: DecimalPrecision | undefined,
    showIssueSeverity: boolean = false,
    warnings: ProjectWarning[] = [],
  ): void {
    // Early bail when the map is gone or its style has been torn down - the
    // coalescer's queueMicrotask can race with MapCanvas unmount → map.remove().
    // A lost WebGL context also has no style, and while a rebuilt style loads
    // (context or worker restore) no layer can be added: keep the args for the
    // restore to replay.
    if (isStyleRestoring(this.map) || !hasLiveStyle(this.map)) {
      this.updateWhileStyleless = [sections, stations, loopClosures, gpsOrigin, showDepthGradient,
        depthRange, precision, showIssueSeverity, warnings];
      return;
    }

    this.coalescedUpdateCallsSinceLastFlush = 0;
    this.gpsOrigin = gpsOrigin;
    this.showDepthGradient = showDepthGradient;
    this.depthRange = normalizeDepthRange(depthRange);
    this.showIssueSeverity = showIssueSeverity;
    this.cachedWarnings = warnings;

    // Sections this flush removes (delete / undo-of-add), read BEFORE the cache
    // overwrite: their custom geometry vanishes synchronously in syncCustomRender
    // below, so a removed feature's stale interact-overlay highlight is dropped in
    // the same frame rather than lingering (#customRender delete straggler).
    const removedSectionIds = diffRemovedSectionIds(this.cachedSections, sections);

    // Cache input data for the theme rebuild / drag-preview / hit-test paths.
    this.cachedSections = sections;
    this.cachedStations = stations;
    this.cachedLoopClosures = loopClosures;
    if (precision !== undefined) this.cachedPrecision = precision;

    // Prefer the live store geometry (never a stale project's) over the legacy
    // cached args; fall back to the args only when the store has no active geometry.
    const surveyData = selectActiveSurveyData(
      useProjectStore.getState(),
      useUIStore.getState().geometryMode
    ) ?? surveyDataFromLegacy(sections, stations);

    const issueSeverity = showIssueSeverity
      ? computeIssueSeverity(surveyData, warnings)
      : undefined;
    this.cachedSurveyData = surveyData;
    this.cachedIssueSeverity = issueSeverity;
    // INVARIANT: geoIndex is re-created (never mutated in place) on every flush -
    // its per-instance lazy caches rest on a fresh instance per geometry change.
    this.geoIndex = new SurveyGeometryIndex(surveyData, this.gpsOrigin);

    // Custom WebGL layers are the SOLE survey renderer: geometry + labels
    // (syncCustomRender) and the measured-path tint (syncCustomMeasurePath) draw
    // straight from the SoA SurveyData, synchronously in this frame. Loop closures
    // ride their own small GeoJSON source (not part of the custom buffers).
    this.syncCustomRender(surveyData);
    this.syncCustomMeasurePath();
    publishLoopClosures(this.map, this.cachedLoopClosures, this.geoIndex);

    // Delete straggler: a selected / hovered / snapped feature this flush removed
    // must drop from the interact overlay now (its custom geometry already vanished
    // above), instead of lingering. Skipped mid-drag (the drag paths own the overlay
    // follow). Hide the interact layers across the overlay GeoJSON source's own async
    // re-tile so the stale highlight drops in the geometry frame.
    if (
      !this.drag.dragPreviewActive &&
      removedSectionIds.length > 0 &&
      (this.selectedShotId || this.hoveredShotId || this.selectedStationId ||
        this.hoveredStationId || this.snappedStationId || this.highlightedStationIds.length > 0)
    ) {
      const stale = overlayTouchesRemovedSections(
        [this.selectedShotId, this.hoveredShotId, this.selectedStationId,
          this.hoveredStationId, this.snappedStationId, ...this.highlightedStationIds],
        removedSectionIds,
      );
      this.lastInteractKey = null;
      this.refreshInteractOverlay();
      if (stale) this.interactHide.hideUntilRetile();
    }

    // Refresh the interact overlay + its paint inputs against the new geometry.
    this.lastInteractKey = null;
    this.applyInteractPaint();
    // Drag-commit atomic settle: drop the follow-offset now the committed geometry
    // is installed, BEFORE the overlay refresh reads it (no snap-back / overshoot).
    if (this.drag.dragCommitPending) this.drag.clearDragOverlayFollow();
    this.updateVisualState();
    // Gas route highlight (#233): re-push the route (coords come from cachedStations).
    if (this.gasRouteLegs.length > 0) applyGasRoute(this.map, this.gasRouteLegs, this.cachedStations, this.gpsOrigin);

    // Fire the one-shot callback (drag commit restores its filters in the same
    // synchronous block that installed the geometry, so there is no paint gap).
    if (this.onNextUpdateCallback) {
      const cb = this.onNextUpdateCallback;
      this.onNextUpdateCallback = null;
      cb();
    }
  }

  /**
   * Register a one-shot callback to run at the end of the next update() call,
   * after all sources have been refreshed. Used to restore drag preview filters
   * in the same synchronous block as the data update (no paint gap).
   */
  onNextUpdate(callback: () => void): void {
    this.onNextUpdateCallback = callback;
  }

  /**
   * Apply new theme colors to all MapLibre paint/layout properties
   * AND rebuild features (because buildSurveyFeatures embeds theme colors in GeoJSON properties).
   * Called from MapCanvas when resolvedTheme changes.
   */
  applyThemeColors(colors: MapThemeColors): void {
    this.themeColors = colors;

    if (!hasLiveStyle(this.map)) return;

    // Overlay features carry resolved theme colors - rebuild them.
    this.lastInteractKey = null;
    this.refreshInteractOverlay();

    // Update the kept auxiliary layers' constant theme colors.
    applyAuxiliaryThemeColors(this.map, colors);

    // Rebuild the survey from cache so the custom renderer + labels + measured-path
    // tint (which embed theme colors) repaint with the new theme.
    if (this.cachedSections.length > 0 || this.cachedStations.length > 0) {
      this.update(
        this.cachedSections, this.cachedStations, this.cachedLoopClosures, this.gpsOrigin,
        this.showDepthGradient, this.depthRange, this.cachedPrecision, this.showIssueSeverity,
        this.cachedWarnings,
      );
    }
  }

  /**
   * Every drawn station center + shot segment in map coords, honoring the live
   * reference-shots toggle. Exposed for E2E: the custom renderer draws survey
   * geometry in a WebGL layer that `queryRenderedFeatures` cannot see, so tests
   * project these instead of querying deleted stock layers (see MapCanvas's
   * `__stalagmight_geometry__` hook). Returns null before geometry is indexed.
   */
  enumerateGeometry(): ReturnType<SurveyGeometryIndex['enumerateGeometry']> | null {
    if (!this.geoIndex) return null;
    return this.geoIndex.enumerateGeometry(this.visibleKinds());
  }

  /**
   * How much measured-path tint is currently drawn (shot segments + station
   * discs). Exposed for E2E: the tint is a custom GL layer with no MapLibre
   * feature-state, so this replaces reading a `measured` feature-state off the
   * deleted vector-tile sources.
   */
  measuredPathDrawnCounts(): { shots: number; stations: number } {
    return this.measurePath?.drawnCounts() ?? { shots: 0, stations: 0 };
  }

  /**
   * The live drag-preview offset in screen px, plus which ghost path is drawing
   * (`custom` while the custom ghost layer is live, else `stock`). Null when no
   * drag preview is active. Exposed for E2E so a drag test reads the offset from
   * a supported accessor instead of the ghost layer's private fields.
   */
  dragGhostOffset(): { mode: 'custom' | 'stock'; dx: number; dy: number } | null {
    const off = this.drag.dragOverlayOffset;
    if (!off) return null;
    return { mode: this.dragGhost.isCustom() ? 'custom' : 'stock', dx: off.dx, dy: off.dy };
  }

  /**
   * The JS-only teardown both routes off the map share: MapCanvas's lightweight
   * unmount (which calls this and then `map.remove()`, deliberately skipping
   * {@link destroy} for unmount speed) and destroy() itself. Map-free on purpose -
   * no layer or source removal, which `map.remove()` does wholesale.
   *
   * Everything here is something that outlives `map.remove()` and would otherwise
   * fire later against the dead map and pin this whole layer (cached SurveyData,
   * geometry index, glyph atlas) until it does: the controllers' window sync hooks
   * (#mem), the DPR watcher, the hover and drag-follow animation frames, the drag
   * ghost's bake timer + land listener, the interact-overlay hide's fallback timer
   * + listener (whose late restore called `map.getLayer` on the removed map and
   * threw, #576 D16), the touch long-press timer + canvas listeners, and the
   * WebGL context-loss watch. Idempotent.
   */
  release(): void {
    this.customRender?.dispose();
    this.customLabels?.dispose();
    this.dprWatcherDispose?.();
    this.dprWatcherDispose = undefined;
    this.hoverFrame?.cancel();
    this.drag.cancelDragRender();
    this.dragGhost.dispose();
    this.interactHide.drop();
    // The helper holds the canvas element itself, so it detaches cleanly even
    // once the map is gone.
    this.detachLongPressTouch?.();
    this.detachLongPressTouch = null;
    this.detachGlContextWatch?.();
    this.detachGlContextWatch = null;
    this.updateWhileStyleless = null;
  }

  destroy() {
    // Cancel any pending coalesced update so its microtask doesn't fire on a
    // destroyed map (would throw in setData).
    this.updateScheduled = false;
    this.pendingUpdateArgs = null;

    // Restore any interact layers left hidden by an in-flight removal re-tile and
    // drop its listener/timer so nothing fires on the torn-down map.
    this.interactHide.dispose();

    // Tear down the custom-render prototype (removes its layer, restores stock
    // layer visibility, and unregisters the runtime sync hook) (#customRender).
    this.customRender?.destroy();
    this.customRender = null;
    this.customLabels?.destroy();
    this.customLabels = null;
    // Remove the dedicated custom measure-path layer (#26, stage 2a).
    this.measurePath?.destroy();
    this.measurePath = null;
    // Remove the ghost (custom layer / stock preview layers) while the map is live.
    this.dragGhost.end();

    // Then everything map-free, shared with MapCanvas's lightweight unmount.
    this.release();

    // Guard against map being destroyed (race condition on unmount)
    if (!hasLiveStyle(this.map)) {
      // Clear bound handler references even if map is destroyed
      this.boundMouseMove = null;
      this.boundMapLeave = null;
      return;
    }

    try {
      // Unregister event handlers first (before removing layers)
      this.map.off('click', this.handleMapClick);
      this.map.off('error', this.handleMapError);
      this.map.off('contextmenu', this.handleContextMenu);

      if (this.boundMouseMove) {
        this.map.off('mousemove', this.boundMouseMove);
      }
      if (this.boundMapLeave) {
        this.map.getCanvas().removeEventListener('mouseout', this.boundMapLeave);
      }
      this.map.off('zoomend', this.handleZoomEndTelemetry);
      this.map.off('moveend', this.handleMoveEndTelemetry);

      // Remove all layers first (must be done before removing their sources).
      // The custom survey geometry / labels / measured-path layers are removed by
      // their controllers' destroy() calls above; only the auxiliary GeoJSON
      // overlays are owned here.
      removeAuxiliaryLayers(this.map);
    } catch {
      // Map already destroyed during unmount, cleanup not needed
    }

    // Clear bound handler references
    this.boundMouseMove = null;
    this.boundMapLeave = null;
  }
}
