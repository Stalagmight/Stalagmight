import type { Map as MaplibreMap, MapMouseEvent, ExpressionSpecification } from 'maplibre-gl';
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
import { hasLiveStyle } from './mapUtils';
import { buildGhostFeatures } from './surveyLayer/ghostFeatures';
import {
  normalizeDepthRange,
  resolveShotDisplayColor,
  resolveStationDisplayColor,
  type DepthRange,
  type DisplayColorContext,
  type GhostSectionData,
} from './surveyLayer/displayColor';
import {
  SurveyGeometryIndex,
  type VisibleKinds,
} from './surveyLayer/geometryIndex';
import {
  type DragPreviewFeature,
  type DragPreviewSet,
  type MovingEnd,
  type Projector,
} from './surveyLayer/dragPreviewSource';
import { DRAG_PREVIEW_SOURCE, type DragPreviewLayerCtx } from './surveyLayer/dragPreviewLayers';
import {
  shotOverlayFeature,
  stationOverlayFeature,
  type InteractOverlayDeps,
} from './surveyLayer/interactOverlayFeatures';
import { DragGhostController } from './surveyLayer/dragGhostController';
import type { GhostRenderData } from './surveyLayer/customRender/surveyGhostLayer';
import { buildGhostRenderData } from './surveyLayer/dragGhostData';
import { padProbeBox, type ProbeGeometry } from './surveyLayer/hitProbe';
import { computeIssueSeverity, type IssueSeverity } from './surveyLayer/severity';
import { SurveyCustomController, buildCustomDimState, type CustomRenderExclusion } from './surveyLayer/customRender/controller';
import { SurveyLabelController } from './surveyLayer/customRender/labelController';
import { overlayInputFrom } from './surveyLayer/customRender/overlayColor';
import { MeasurePathController } from './surveyLayer/customRender/measurePathController';
import { isStyleRestoring, onStyleRestored } from './surveyLayer/customRender/styleRestore';
import type { MeasurePathRenderData } from './surveyLayer/customRender/surveyMeasurePathLayer';
import {
  buildMeasurePathBuffers,
  type MeasureSegment,
  type MeasurePoint,
} from './surveyLayer/customRender/measurePathTessellate';
import { packCssColor } from './surveyLayer/customRender/colorPack';
import type { SurveyData } from '../../types/survey-data';
import type { ProjectWarning } from '../../types/commands';
import { getMapThemeColors, type MapThemeColors } from '../../utils/themeColors';
import {
  distanceColorExpression,
  DISTANCE_NEAR,
  DISTANCE_FAR,
  type DistanceFeatureState,
} from '../../utils/distanceGradient';
import {
  gasReachColorExpression,
  GAS_NEAR,
  GAS_FAR,
  type GasReachFeatureState,
} from '../../utils/gasReach';
import { metersToMapCoords } from '../../utils/coordinates';
import { getErrorColor } from '../../utils/colors';
import { trackNav } from '../../utils/telemetry';
import { FEATURE_ID_STRIDE, sectionIdFromKey } from '../../utils/ids';
import { MEASURE_TINT } from '../../utils/measurePath';
import { watchDevicePixelRatio } from '../../utils/dprWatcher';
import { attachLongPressTouch } from '../../utils/longPressTouch';
import { clearFeatureCountsForPrefix } from '../../utils/memoryProbe';
import { shouldSuppressMapError, type MapErrorEventLike } from './surveyLayer/mapErrors';
import { diffRemovedSectionIds } from './surveyLayer/sectionDiff';
import { InteractOverlayHideController, overlayPaintDigest, overlayTouchesRemovedSections } from './surveyLayer/interactOverlayHide';
import type { SemanticMapTarget } from './surveyLayer/semanticTarget';
import { SemanticTargetPreference } from './surveyLayer/semanticTargetPreference';

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
  private sourceId = 'survey-data';
  // Stable ids kept only as z-order fallback anchors for the custom measure-path
  // layer's beforeId (the stock layers themselves are gone).
  private stationsLayerId = 'survey-stations';
  private loopClosuresLayerId = 'survey-loop-closures';
  private ghostShotsLayerId = 'survey-ghost-shots';
  private ghostStationsLayerId = 'survey-ghost-stations';
  private ghostMergeLayerId = 'survey-ghost-merge';
  private stationsHighlightLayerId = 'survey-stations-highlight';
  // Interact overlay (#mem): a tiny GeoJSON source holding ONLY the currently
  // hovered / selected / snapped shot(s)/station(s) as individual features.
  // With merged bulk features, per-item hover/selection can no longer use
  // per-feature feature-state; instead these few features are redrawn above
  // the bulk layers with the same resolved paint values, so pixels match.
  private interactSourceId = 'survey-interact';
  private interactShotsLayerId = 'survey-interact-shots';
  private interactShotsMeasuredLayerId = 'survey-interact-shots-measured';
  private interactStationsLayerId = 'survey-interact-stations';
  // NOTE: the former dedicated survey-shots-hitbox / survey-stations-hitbox
  // layers were removed (#mem budget). Each was a full unfiltered duplicate
  // vertex bucket of its bulk source (~75k line segments / ~90k points) built
  // solely to gate hit-testing. The candidate-section gate now queries the
  // VISIBLE shots/stations layers with a padded probe box that reproduces the
  // old hit tolerance exactly - see hitCandidateSections().
  // Gas route highlight (#233): the picked penetration route drawn over the survey - one line
  // layer, swum legs at full opacity and jump legs faded via a data-driven line-opacity (#243).
  private gasRouteLayerId = 'survey-gas-route';

  private ghostStationsHitboxLayerId = 'survey-ghost-stations-hitbox';
  private ghostShotsHitboxLayerId = 'survey-ghost-shots-hitbox';
  private callbacks: SurveyLayerCallbacks;
  private semanticTargetPreference = new SemanticTargetPreference({ geometry: () => this.enumerateGeometry(), project: (lngLat) => this.map.project(lngLat), visibility: () => ({ stations: this.stationsVisible, shots: this.shotsVisible }), callbacks: () => this.callbacks });

  // Config storage with fallback defaults for when config is null (initial load)
  private config: Config | null = null;
  /** Stroke ring width of a station marker, px (zero-datum: 2x, like the radius). */
  private static readonly STATION_STROKE_PX = 1;

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

  /**
   * The RESTING cursor for the active tool (zoom-in, grab, crosshair …), pushed in
   * by MapCanvas, which owns that decision - including the Ctrl-held grab, which
   * this class has no way to know about.
   *
   * Exists because this class also writes `canvas.style.cursor`, and used to write
   * a bare `''` whenever nothing was hovered. Both writers are legitimate (only
   * MapCanvas knows the tool, only this knows what is under the pointer and whether
   * it is anchored/immovable), but `''` is not "no opinion" - it is "default arrow",
   * so it silently erased the tool cursor MapCanvas had just set. The clobber won
   * because it lands a render LATER: MapCanvas's cursor effect is passive, and the
   * dim-set update that follows a tool switch re-runs this from a layout effect,
   * by which time the passive effect has already run and will not run again (#24).
   */
  private baseCursor = '';

  /**
   * Whether {@link baseCursor} is a MODE cursor (the Ctrl-held pan grab) rather
   * than the tool's own resting one. A mode outranks the Move Station/Shot arrow
   * below: that arrow exists only to stop MapLibre's *default* grab showing
   * through off-feature, so letting it beat a grab the user is deliberately
   * holding Ctrl for inverts what it is there to do.
   */
  private baseCursorForced = false;

  // Anchored section IDs (for fading when Move Segment tool is active)
  private anchoredSectionIds: Set<number> = new Set();

  // Immovable feature IDs for Move Station / Move Shot tools
  // (per-feature granularity, unlike anchoredSectionIds which is per-section)
  private immovableStationFeatureIds: Set<number> = new Set();
  private immovableShotFeatureIds: Set<number> = new Set();

  // Drag preview state - uses temporary layers with *-translate paint properties
  // instead of rebuilding GeoJSON on every frame
  private dragPreviewActive = false;
  // When MapCanvas last confirmed a placement (performance.now() ms); see
  // modalInteractionActive for why the store read alone cannot see it.
  private lastPlacementConfirmAt = -Infinity;
  private dragSavedFilters = new Map<string, maplibregl.FilterSpecification | undefined>();
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
  /** Coalesced-call counter, surfaced to memory probe for instrumentation. */
  private coalescedUpdateCallsSinceLastFlush = 0;
  // Unified drag-preview source (#mem drag fixes): ONE GeoJSON source holding
  // every moving GEOMETRY feature for the active drag - stations (points), shots
  // (lines), loop-closure indicator lines - rebuilt from captured originals each
  // frame and pushed via a SINGLE setData. Real moved coordinates (not
  // `*-translate`) so the tiler clips moved geometry - no background wedges at
  // large offsets (BUG A); one setData so stations/shots can't skew apart (BUG B).
  // Attached text is not previewed (it is hidden from the stock text layers).
  // The moving ghost is owned by {@link DragGhostController}; DRAG_PREVIEW_SOURCE
  // is imported only for setDragExclusionWhenPreviewReady's stock-preview load gate.
  /** Captured moving geometry for the active drag (originals + movingEnd). */
  private dragPreviewSet: DragPreviewSet | null = null;
  /** rAF coalescer for the per-frame interact-overlay follow: MapCanvas may push
   *  several offsets per frame (pointermove + the map-'move' re-apply), but the
   *  overlay only needs the latest - one small setData per frame. (The GHOST is
   *  driven synchronously by {@link DragGhostController}, decoupled from this rAF.) */
  private dragUpdateRaf: number | null = null;
  private pendingDragOffset: { dx: number; dy: number } | null = null;
  /** Drag ghost (#mem drag latency): owns the moving-geometry preview and drives
   *  it from a live screen-px offset. The CUSTOM path renders a WebGL layer whose
   *  drag delta is a single vertex-shader uniform (no worker, no re-bake, so no
   *  overshoot); the STOCK fallback is the historical GeoJSON preview +
   *  single-flight {@link GhostFollow} bake / `line-translate` machine. See
   *  {@link DragGhostController}. Constructed once, reused across drags. */
  private dragGhost!: DragGhostController;
  /** Interact-overlay follow (#C): a hover/selected highlight on a moving
   *  station/shot is re-emitted at the drag offset each frame so it tracks the
   *  cursor. Stationary overlay features (snap ring, highlightedStationIds,
   *  unrelated selections) keep their true coordinates. */
  private dragMovingStationKeys = new Set<string>();
  private dragMovingShots = new Map<string, MovingEnd>();
  private dragOverlayOffset: { dx: number; dy: number } | null = null;
  /** Set by commitDragPreview at placement: the follow-offset (dragOverlayOffset +
   *  moving sets) is deliberately KEPT applied across the IPC + flush window so the
   *  grabbed highlight stays on the cursor instead of snapping back to the OLD
   *  coords. endApplyFeatures drops it atomically, right before the post-commit
   *  overlay refresh reads the freshly installed geometry (no snap-back, and no
   *  old+offset overshoot). */
  private dragCommitPending = false;
  /** Deferred custom-render drag-hide (START-flash fix). The moving originals are
   *  hidden from the custom WebGL layer only once the drag-preview GeoJSON source
   *  has parsed and drawn its first frame - the preview's setData is async (~5
   *  frames on a real project) while setDragHide is a synchronous GPU upload, so
   *  hiding eagerly leaves a window where the geometry is in NEITHER the custom
   *  layer nor the (not-yet-parsed) preview: a blank flash. Stored so it can be
   *  force-applied at commit and torn down on end; `dragGeneration` is bumped per
   *  drag so a late sourcedata callback from a superseded drag is ignored. */
  private dragGeneration = 0;
  private pendingDragHide: {
    exclusion: CustomRenderExclusion;
    listener: (e: maplibregl.MapSourceDataEvent) => void;
  } | null = null;
  /** Hides the interact-overlay layers across a removal flush's async re-tile so a
   *  deleted feature's hover/selection highlight drops in the geometry frame
   *  instead of lingering on the stale GeoJSON tile. See interactOverlayHide.ts. */
  private readonly interactHide: InteractOverlayHideController;

  // Visibility state (for hidden feature detection)
  private stationsVisible: boolean = true;
  private shotsVisible: boolean = true;

  // GPS origin for coordinate transforms (enables satellite tile alignment)
  private gpsOrigin: GpsOrigin | null = null;

  // Depth gradient mode state (used during update() for feature color calculation)
  private showDepthGradient: boolean = false;
  private depthRange: DepthRange = { min: 0, max: 50 };

  // Issue-severity coloring mode (#22). Like showDepthGradient, cached so the
  // lazy-labels zoom rebuild and theme rebuild re-fire with the same mode.
  private showIssueSeverity: boolean = false;
  private cachedWarnings: ProjectWarning[] = [];

  // Distance-gradient overlay (#34). feature-state based so re-root needs no
  // feature rebuild; re-applied after source rebuilds because endApplyFeatures
  // clears state before tile IDs can be reused for different merged groups.
  private distanceMode: boolean = false;
  private distanceState: DistanceFeatureState | null = null;
  // Gas reachable-frontier overlay (#38). Same feature-state mechanics as the
  // distance gradient (own `gasReach` key); mutually exclusive with it (both own
  // the shots/stations color paint), enforced upstream in uiStore.
  private gasReachMode: boolean = false;
  private gasReachState: GasReachFeatureState | null = null;
  // Plan-mode idle tint (#395 revision): true while Plan is active but no gas
  // origin has been chosen yet - the whole survey renders the gas ramp's dim far
  // color instead of leaking section colors under the gas scheme. Driven by
  // MapCanvas (setPlanIdleTint); yields to a real gas field via isPlanIdleActive().
  private planIdleTint: boolean = false;
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

  // Hover update batching - only process latest mouse position per frame
  private pendingHoverEvent: maplibregl.MapMouseEvent | null = null;
  private hoverRafId: number | null = null;

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
  /** Dispose for the re-add after a style rebuild (context / worker restore). */
  private styleRestoreDispose: (() => void) | undefined;
  /** The last update that arrived while a restore was under way; replayed once
   *  the rebuilt style has loaded. */
  private deferredUpdate: (() => void) | null = null;

  constructor(map: MaplibreMap, callbacks: SurveyLayerCallbacks = {}) {
    this.map = map;
    this.callbacks = callbacks;
    this.interactHide = new InteractOverlayHideController(map, this.interactSourceId, [
      this.interactShotsLayerId,
      this.interactShotsMeasuredLayerId,
      this.interactStationsLayerId,
      this.stationsHighlightLayerId,
    ]);
    this.initializeSources();
    this.initializeLayers();
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
    this.measurePath = new MeasurePathController(map, {
      onFatal: (reason) => this.handleRenderFatal(reason),
    });
    // The custom layers bake devicePixelRatio into their device-pixel sizes, so a
    // DPR change (window dragged to a different-scale monitor) must rebuild them.
    this.dprWatcherDispose = watchDevicePixelRatio(() => this.handleDprChange());
    this.styleRestoreDispose = onStyleRestored(map, () => this.reattachCustomLayers());
  }

  /** MapLibre rebuilt the style without the custom layers; put them back in
   *  their original z-order (geometry, measure tint above it, labels, then the
   *  drag ghost on top). */
  private reattachCustomLayers(): void {
    this.customRender?.reattach();
    this.measurePath?.reattach();
    this.customLabels?.reattach();
    this.dragGhost.reattach();
    const update = this.deferredUpdate;
    this.deferredUpdate = null;
    update?.();
    // Overlay changes made while the style was gone never reached the rebuilt
    // one (the snapshot predates them); redraw the ones derived from live state.
    this.lastInteractKey = null;
    this.refreshInteractOverlay();
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
    this.updateHitboxLayers();
    this.updateGhostVisualLayers();
    // Overlay features carry config-derived widths/radii - force a rebuild.
    this.lastInteractKey = null;
    this.updateVisualState();
    this.updateDebugVisualization();
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

    // One derivation for every tool, from the state just updated above: the arrow
    // Move Station/Shot need before any hover event fires, the pointer if something
    // is already under the cursor, and otherwise the tool's own resting cursor.
    // These three cases used to be written out here as literals - including two
    // bare `''`, which wiped the tool cursor MapCanvas had set for exactly the
    // tools (pan, zoom) whose cursor IS the affordance (#24).
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

  private initializeSources() {
    // Custom-only renderer: survey geometry + labels + measured-path tint are
    // drawn by the custom WebGL layers (SurveyCustomController /
    // SurveyLabelController / MeasurePathController) straight from the SoA
    // SurveyData - there are no stock bulk vector-tile sources for shots /
    // stations / shot-gradients / labels anymore. Only the small GeoJSON
    // auxiliaries (loop closures, gas route, merge ghosts, interact overlay)
    // live here.

    // Loop closures line source
    this.map.addSource(this.sourceId + '-loop-closures', {
      type: 'geojson',
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    // Gas route highlight line source (#233): one LineString per route leg, `isJump` flagged.
    this.map.addSource(this.sourceId + '-gas-route', {
      type: 'geojson',
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    // Ghost section sources for merge preview
    this.map.addSource(this.sourceId + '-ghost-shots', {
      type: 'geojson',
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    this.map.addSource(this.sourceId + '-ghost-stations', {
      type: 'geojson',
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    this.map.addSource(this.sourceId + '-ghost-merge', {
      type: 'geojson',
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    // Interact overlay source (#mem): holds only the handful of currently
    // hovered / selected / snapped features as individual GeoJSON features
    // (see refreshInteractOverlay). Bounded by interaction state, not survey
    // size, so its retained cost is negligible.
    this.map.addSource(this.interactSourceId, {
      type: 'geojson',
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });
  }

  private initializeLayers() {
    // === HIT-TEST GATING ===
    // The dedicated survey-shots-hitbox / survey-stations-hitbox layers were
    // removed (#mem budget): each was a full unfiltered duplicate vertex bucket
    // of its bulk source. Hit-testing now queries the VISIBLE shots/stations
    // layers with a padded probe box (see hitCandidateSections). The ghost
    // hitbox layers below stay - they live on the tiny ghost sources and drive
    // the merge-preview debug visualization only.
    //
    // Layer order (bottom to top):
    // 1. survey-ghost-shots-hitbox (debug only)
    // 2. survey-ghost-stations-hitbox (debug only)
    // Then visual layers follow...

    // Ghost shot hitbox layer (for debug visualization only - NOT interactive)
    this.map.addLayer({
      id: this.ghostShotsHitboxLayerId,
      type: 'line',
      source: this.sourceId + '-ghost-shots',
      layout: {
        'line-cap': 'round',
      },
      paint: {
        'line-width': this.getShotHitArea(),
        'line-color': 'rgba(255, 255, 0, 0.3)', // Visible when debug enabled
        'line-opacity': 0, // Hidden by default, shown via updateDebugVisualization
      },
    });

    // Ghost station hitbox layer (for debug visualization only - NOT interactive)
    this.map.addLayer({
      id: this.ghostStationsHitboxLayerId,
      type: 'circle',
      source: this.sourceId + '-ghost-stations',
      paint: {
        'circle-radius': this.getStationHitArea(),
        'circle-color': 'rgba(255, 255, 0, 0.3)', // Visible when debug enabled
        'circle-opacity': 0, // Hidden by default, shown via updateDebugVisualization
        'circle-stroke-width': 0,
      },
    });

    // === VISUAL LAYERS ===

    // Ghost section shots layer (dashed, semi-transparent) - rendered first so it's behind everything
    // Uses config-based line width, updated via updateGhostVisualLayers()
    this.map.addLayer({
      id: this.ghostShotsLayerId,
      type: 'line',
      source: this.sourceId + '-ghost-shots',
      paint: {
        'line-color': ['get', 'color'],
        'line-width': this.getShotWidth(),
        'line-opacity': 0.5,
        'line-dasharray': [4, 2],
      },
    });

    // Ghost section stations layer (semi-transparent)
    // Uses config-based radius, updated via updateGhostVisualLayers()
    this.map.addLayer({
      id: this.ghostStationsLayerId,
      type: 'circle',
      source: this.sourceId + '-ghost-stations',
      paint: {
        'circle-radius': this.getStationRadius(),
        'circle-color': ['get', 'color'],
        'circle-stroke-width': 1,
        'circle-stroke-color': this.themeColors.strokeColor,
        'circle-opacity': 0.5,
      },
    });

    // Ghost merge station highlight layer
    this.map.addLayer({
      id: this.ghostMergeLayerId,
      type: 'circle',
      source: this.sourceId + '-ghost-merge',
      paint: {
        'circle-radius': 8,
        'circle-color': this.themeColors.selectHighlight,
        'circle-stroke-width': 2,
        'circle-stroke-color': this.themeColors.strokeColor,
        'circle-opacity': 0.8,
      },
    });

    // Loop closures layer (dashed lines with color based on error severity)
    this.map.addLayer({
      id: this.loopClosuresLayerId,
      type: 'line',
      source: this.sourceId + '-loop-closures',
      paint: {
        // Color based on error percentage: green (<1%), yellow (1-3%), orange (3-5%), red (>5%)
        'line-color': ['get', 'color'],
        'line-width': 3,
        'line-opacity': 0.8,
        'line-dasharray': [6, 4], // Dashed line pattern for loop closures
      },
    });

    // === OVERLAY LAYERS (above the custom-rendered survey geometry) ===
    // Custom-only renderer: survey shots / stations / shot-gradients / labels /
    // comments / measured-path tint are drawn by the custom WebGL layers (added
    // by their controllers), NOT stock MapLibre layers. What remains here are the
    // small auxiliary overlays that sit ABOVE the custom geometry:
    //   - snap-station highlight + interact hover/selection overlay (interact source),
    //   - gas-route highlight.
    // The custom geometry layer inserts itself before `survey-interact-shots`
    // (see SurveyCustomController.enable), so it lands above loop closures /
    // ghosts and below every overlay added below. The custom label layer draws on
    // top of everything; the custom measure-path layer inserts before
    // `survey-interact-shots-measured` (see syncCustomMeasurePath).

    // Station highlight layer for snapped/active stations (interact source).
    // Snapped stations are pushed to the interact overlay as individual
    // `kind: 'snap'` features (setSnappedStation / setHighlightedStations).
    this.map.addLayer({
      id: this.stationsHighlightLayerId,
      type: 'circle',
      source: this.interactSourceId,
      filter: ['==', ['get', 'kind'], 'snap'],
      paint: {
        'circle-radius': this.getStationRadius() + 3, // Snapped state: radius + 3
        'circle-color': this.themeColors.snapColor,
        'circle-stroke-width': 4,
        'circle-stroke-color': this.themeColors.snapColor,
        // Disable default 300ms paint transitions - snap indicator must appear/disappear instantly
        'circle-opacity-transition': { duration: 0, delay: 0 },
        'circle-stroke-opacity-transition': { duration: 0, delay: 0 },
      },
    });

    // Gas route highlight (#233): the picked route drawn over the survey. Jump legs are
    // faded rather than dashed (#243) - the same half-opacity treatment reference shots
    // get - and unlike `line-dasharray`, `line-opacity` IS data-driven, so one layer
    // covers both leg kinds.
    this.map.addLayer({
      id: this.gasRouteLayerId,
      type: 'line',
      source: this.sourceId + '-gas-route',
      paint: {
        'line-color': ['coalesce', ['get', 'segmentColor'], '#22d3ee'],
        'line-width': 4,
        'line-opacity': ['case', ['==', ['get', 'isJump'], true], 0.45, 0.9],
      },
    });

    // === INTERACT OVERLAY LAYERS (#mem) ===
    // Individual hover/selection highlights, drawn from the tiny interact source,
    // stacked bottom-to-top: interact-shots (hover/select shot), then the measured
    // hover tint, then interact-stations, then the snap highlight. They are added
    // with explicit beforeIds so this relative order holds and the custom geometry
    // layer (inserted before interact-shots) sits beneath all of them. Colors are
    // resolved per feature at overlay-build time or via the same ramp expressions
    // in overlay modes.
    this.map.addLayer({
      id: this.interactStationsLayerId,
      type: 'circle',
      source: this.interactSourceId,
      filter: ['==', ['get', 'kind'], 'station'],
      paint: {
        'circle-radius': ['get', 'radius'],
        'circle-color': ['get', 'color'],
        'circle-opacity': ['get', 'opacity'],
        'circle-stroke-width': ['get', 'strokeWidth'],
        'circle-stroke-color': ['get', 'strokeColor'],
        'circle-stroke-opacity': ['get', 'opacity'],
      },
    }, this.stationsHighlightLayerId);

    this.map.addLayer({
      id: this.interactShotsMeasuredLayerId,
      type: 'line',
      source: this.interactSourceId,
      filter: ['==', ['get', 'kind'], 'shotMeasured'],
      layout: {
        'line-cap': 'round',
        'line-join': 'round',
      },
      paint: {
        'line-color': ['get', 'color'],
        'line-width': ['get', 'width'],
        'line-opacity': ['get', 'opacity'],
      },
    }, this.interactStationsLayerId);

    this.map.addLayer({
      id: this.interactShotsLayerId,
      type: 'line',
      source: this.interactSourceId,
      filter: ['==', ['get', 'kind'], 'shot'],
      paint: {
        'line-color': ['get', 'color'],
        'line-width': ['get', 'width'],
        'line-opacity': ['get', 'opacity'],
      },
    }, this.interactShotsMeasuredLayerId);
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

    // Mousemove handler with RAF batching
    // Only processes the latest mouse position per animation frame,
    // preventing event queue buildup when queryRenderedFeatures is slow
    this.boundMouseMove = (e: maplibregl.MapMouseEvent) => {
      if (isNonInteractiveTool(this.activeTool)) return;
      if (this.dragPreviewActive) return; // Suppress hover during active drag

      // Store latest event (overwrites any pending)
      this.pendingHoverEvent = e;

      // Schedule processing if not already scheduled
      if (this.hoverRafId === null) {
        this.hoverRafId = requestAnimationFrame(() => {
          this.hoverRafId = null;
          if (this.pendingHoverEvent) {
            const event = this.pendingHoverEvent;
            this.pendingHoverEvent = null;
            processHoverUpdate(event);
          }
        });
      }
    };

    // Clear all hover when cursor leaves the map canvas
    this.boundMapLeave = () => {
      if (this.hoveredStationId || this.hoveredShotId) {
        if (this.shouldLogInteraction()) {
          console.debug('[HitDetection] Hover cleared on map leave', {
            previousHoveredStationId: this.hoveredStationId,
            previousHoveredShotId: this.hoveredShotId,
          });
        }
        this.hoveredStationId = null;
        this.hoveredShotId = null;
        // Fall back to the TOOL's resting cursor, not to `''` (#316). Writing
        // `''` here left a bare arrow behind that nothing restored: on re-entry
        // `processHoverUpdate` only calls `refreshHoverCursor` when the hovered
        // id CHANGES, and this handler already nulled it, so the measure
        // crosshair / merge cell / comment crosshair stayed lost until the
        // pointer happened to hover a feature.
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
    this.map.getCanvas().addEventListener('mouseleave', this.boundMapLeave);

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
      this.dragPreviewActive ||
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

  /** Track user-initiated zoom (scroll wheel, pinch, double-click). Skip programmatic zooms. */
  private handleZoomEndTelemetry = (e: maplibregl.MapLibreEvent): void => {
    const orig = e.originalEvent;
    if (!orig) return; // programmatic - already tracked by trigger functions
    if (orig instanceof WheelEvent) {
      trackNav('zoom', 'mouse');
    } else if (typeof TouchEvent !== 'undefined' && orig instanceof TouchEvent) {
      trackNav('zoom', 'touch');
    } else if (orig instanceof MouseEvent) {
      trackNav('zoom', 'dblclick');
    }
  };

  /** Track user-initiated pan (click-drag, touch-drag). Skip programmatic pans and zoom-induced moves. */
  private handleMoveEndTelemetry = (e: maplibregl.MapLibreEvent): void => {
    const orig = e.originalEvent;
    if (!orig) return; // programmatic - already tracked by trigger functions
    if (orig instanceof WheelEvent) return; // zoom gesture - tracked by zoomend
    if (orig instanceof MouseEvent) {
      trackNav('pan', 'mouse');
    } else if (typeof TouchEvent !== 'undefined' && orig instanceof TouchEvent) {
      trackNav('pan', 'touch');
    }
  };

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
    if (shouldSuppressMapError(evt, this.sourceId)) return;
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
          layer: this.stationsLayerId,
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
          layer: this.stationsLayerId,
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

  /**
   * Candidate sections under a point/box, from the SoA geometry index
   * (#customRender / #mem). Candidate discovery no longer depends on stock-layer
   * residency: the endgame renders the survey via the custom WebGL layer, which
   * hides the stock shots/stations layers (visibility:none), and MapLibre returns
   * nothing from hidden layers. So instead of `queryRenderedFeatures` we ask the
   * geometry index for sections whose world-space bounds, projected to screen,
   * intersect the probe box padded by the type's hit reach. That padded-box query
   * is a SUPERSET of the sections whose geometry can be within tolerance; the SoA
   * resolver (`resolveStationAt` / `resolveShotAt`) then trims it to the exact
   * pick. Per-type padding preserves the two old queries' semantics (shot vs
   * station reach differ); the pad is the FULL reach
   * because we test the true geometry, not a thin rendered mark. See
   * `hitProbe.ts` (`padProbeBox`) and `geometryIndex.ts`
   * (`sectionsIntersectingBox`), both unit tested.
   *
   * A type is gated by the USER visibility flags (`stationsVisible` /
   * `shotsVisible`, set by setStationsVisibility / setShotsVisibility): a
   * user-hidden type yields no candidates, exactly as the old query returned
   * nothing for a hidden layer, so clicks fall through to the hidden-feature
   * toast in `handleEmptyClick`. These flags are NOT raw layer visibility: the
   * custom renderer hiding the stock layers (visibility:none) leaves the survey
   * fully interactive, while a user "hide stations/shots" toggle still suppresses
   * hits.
   */
  private hitCandidateSections(
    geometry: maplibregl.PointLike | [maplibregl.PointLike, maplibregl.PointLike],
  ): { stationSections: Set<number>; shotSections: Set<number> } {
    let stationSections = new Set<number>();
    let shotSections = new Set<number>();
    if (!this.geoIndex || !hasLiveStyle(this.map)) return { stationSections, shotSections };
    const project = this.projector();
    // One camera snapshot for both probes: the geometry index reuses its
    // projected per-section AABBs across probes of the same camera, so a static
    // hover reprojects the 14k+ sections once, not once per probe (#customRender).
    const camera = this.cameraSnapshot();
    if (this.shotsVisible) {
      const [[minX, minY], [maxX, maxY]] = padProbeBox(
        geometry as ProbeGeometry,
        this.getShotHitReach(),
      );
      shotSections = this.geoIndex.sectionsIntersectingBox(project, minX, minY, maxX, maxY, camera);
    }
    if (this.stationsVisible) {
      const [[minX, minY], [maxX, maxY]] = padProbeBox(
        geometry as ProbeGeometry,
        this.getStationHitReach(),
      );
      stationSections = this.geoIndex.sectionsIntersectingBox(project, minX, minY, maxX, maxY, camera);
    }
    return { stationSections, shotSections };
  }

  /** Cheap camera snapshot keying the geometry index's projected-AABB cache.
   *  Keyed by the EXACT center (plus zoom / bearing / pitch / canvas size), so
   *  the key changes on ANY camera move; axisAligned (no bearing/pitch) enables
   *  the 2-corner reprojection fast path. The center must stay full-precision:
   *  quantizing it to pixel bins let two centers in one bin differ by ~1px (Web
   *  Mercator amplifies the latitude error away from the equator), exceeding the
   *  probe's 0.5px margin, so a feature near the hit-radius boundary then missed
   *  hover/click until the camera crossed a bin. */
  private cameraSnapshot(): { key: string; axisAligned: boolean } {
    const c = this.map.getCenter();
    const cv = this.map.getCanvas();
    const bearing = this.map.getBearing();
    const pitch = this.map.getPitch();
    const key = `${this.map.getZoom()}|${c.lng}|${c.lat}|${bearing}|${pitch}|${cv.width}|${cv.height}`;
    return { key, axisAligned: bearing === 0 && pitch === 0 };
  }

  /** Screen projection closure for the geometry resolver. */
  private projector(): (c: [number, number]) => { x: number; y: number } {
    return (c) => this.map.project(c);
  }

  /** Nearest visible station among candidate sections, within maxDistance px;
   *  on overlapping markers, the real-line one drawn on top (see
   *  `SurveyGeometryIndex.nearestStation`). Hover, click, context menu and the
   *  measure snap all resolve here, so they agree. */
  private resolveStationAt(
    x: number,
    y: number,
    sections: Set<number>,
    maxDistance: number,
  ): ReturnType<SurveyGeometryIndex['nearestStation']> {
    if (sections.size === 0 || !this.geoIndex) return null;
    return this.geoIndex.nearestStation(this.projector(), x, y, sections, maxDistance, {
      visibleKinds: this.visibleKinds(),
      stationDiscPx: this.getStationRadius() + SurveyLayer.STATION_STROKE_PX,
    });
  }

  /** Nearest visible shot among candidate sections, within maxDistance px. */
  private resolveShotAt(
    x: number,
    y: number,
    sections: Set<number>,
    maxDistance: number,
  ): ReturnType<SurveyGeometryIndex['nearestShot']> {
    if (sections.size === 0 || !this.geoIndex) return null;
    return this.geoIndex.nearestShot(this.projector(), x, y, sections, maxDistance, {
      visibleKinds: this.visibleKinds(),
    });
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

  /** Point/arrow cursor for whatever is hovered RIGHT NOW, suppressed for anchored
   *  or immovable features. Derived purely from current state so it can be re-run
   *  when a dim set changes, not only when the hovered id changes - the anchored
   *  and immovable sets arrive asynchronously, and without a recompute the cursor
   *  disagrees with the highlight until the pointer leaves the feature (#301). */
  /**
   * Tell this layer the active tool's resting cursor, and re-derive from it.
   *
   * `forced` marks a mode cursor (the Ctrl-held pan grab) that must also outrank
   * the Move Station/Shot arrow - see {@link baseCursorForced}.
   */
  public setBaseCursor(cursor: string, forced = false): void {
    this.baseCursor = cursor;
    this.baseCursorForced = forced;
    this.refreshHoverCursor();
  }

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
    const useArrowFallback = this.activeTool === 'move_station' || this.activeTool === 'move_shot';
    if (showPointer && useArrowFallback && hoveredId) {
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
    // Nothing hovered falls back to the TOOL's resting cursor, not to `''`: this
    // runs again on every dim-set change, so writing `''` here reset the zoom
    // magnifier / pan hand / measure crosshair to a plain arrow moments after the
    // tool was chosen. Move Station/Shot keep their explicit arrow, which is not
    // the tool's own `move` cursor - it exists to stop MapLibre's grab showing
    // through when the pointer is not over a movable feature.
    this.map.getCanvas().style.cursor = showPointer
      ? 'pointer'
      : useArrowFallback && !this.baseCursorForced
        ? 'default'
        : this.baseCursor;
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
    const source = this.map.getSource(this.interactSourceId);
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
    if (this.dragOverlayOffset === null && key === this.lastInteractKey) return;
    this.lastInteractKey = key;

    const features: GeoJSON.Feature[] = [];

    // While the custom ghost is active, the grabbed MOVING feature's highlight is
    // drawn IN the ghost (shares its offset uniform, no lag), so suppress the
    // per-frame overlay copy for moving features; non-moving overlay state stays.
    const suppress = this.dragGhost.isCustom();
    const keepShot = (id: string | null): string | null =>
      suppress && id && this.dragMovingShots.has(id) ? null : id;
    const keepStation = (id: string | null): string | null =>
      suppress && id && this.dragMovingStationKeys.has(id) ? null : id;

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
      planIdleTint: this.isPlanIdleActive(),
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
      isPlanIdleActive: this.isPlanIdleActive(),
      displayColorContext: this.displayColorContext(),
      themeColors: this.themeColors,
      distanceMode: this.distanceMode,
      distanceState: this.distanceState,
      gasReachMode: this.gasReachMode,
      gasReachState: this.gasReachState,
      dragOverlayOffset: this.dragOverlayOffset,
      dragMovingShots: this.dragMovingShots,
      dragMovingStationKeys: this.dragMovingStationKeys,
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
    this.distanceMode = enabled;
    this.distanceState = state;
    if (!this.map || !hasLiveStyle(this.map)) return;
    // Interact overlay follows the mode swap; the custom renderer (no feature-state)
    // rebuilds the survey geometry with the new overlay colors.
    this.applyOverlayPaint();
    this.resyncCustomRenderFromStore();
  }

  /**
   * Apply the gas reachable-frontier overlay (#38). Mirrors
   * {@link setDistanceGradient}: `enabled` swaps the shots/stations color to the
   * gas ramp (gold near the origin → purple at the turn budget, grey =
   * unreachable); `state` carries per-feature `gasReach` (litres-to-reach).
   * Re-running on an input change just calls this again - no feature rebuild.
   */
  setGasReach(enabled: boolean, state: GasReachFeatureState | null): void {
    this.gasReachMode = enabled;
    this.gasReachState = state;
    if (!this.map || !hasLiveStyle(this.map)) return;
    // Interact overlay follows the mode swap; the custom renderer rebuilds the
    // survey geometry with the new overlay colors.
    this.applyOverlayPaint();
    this.resyncCustomRenderFromStore();
    // Beyond-gas-range dimming (#395) follows the same mode/state - toggling
    // Plan mode (or a budget change) applies/clears the gas-dim set.
    this.syncCustomDimming();
  }

  /** True when the plan-idle tint should actually paint: the flag is set AND no
   *  real gas field is active - a real field always wins (#395 revision). */
  private isPlanIdleActive(): boolean {
    return this.planIdleTint && !(this.gasReachMode && this.gasReachState);
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
    if (this.planIdleTint === enabled) return;
    this.planIdleTint = enabled;
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
    this.applyGasRoute();
  }

  /** Build one LineString per route leg (with `isJump` flagged) from `cachedStations` and push them
   *  to the gas-route source. No-op without a live style or the source. */
  private applyGasRoute(): void {
    if (!this.map || !hasLiveStyle(this.map)) return;
    const source = this.map.getSource(this.sourceId + '-gas-route');
    if (!source || source.type !== 'geojson') return;
    const geojsonSource = source as maplibregl.GeoJSONSource;
    // Index the stations once (keyed by section:index) so the per-leg endpoint lookups are O(1) -
    // the route can be long and this re-runs after every setData() rebuild, so a linear scan per
    // endpoint would be quadratic on large surveys.
    const coordByRef = new Map<string, [number, number]>();
    for (const s of this.cachedStations) {
      coordByRef.set(
        `${s.station_ref.section_id}:${s.station_ref.station_index}`,
        metersToMapCoords(s.coordinates.x, s.coordinates.y, this.gpsOrigin) as [number, number],
      );
    }
    const coordFor = (ref: StationRef): [number, number] | null =>
      coordByRef.get(`${ref.section_id}:${ref.station_index}`) ?? null;
    const features = this.gasRouteLegs.flatMap((leg) => {
      const from = coordFor(leg.from);
      const to = coordFor(leg.to);
      if (!from || !to) return [];
      return [
        {
          type: 'Feature' as const,
          geometry: { type: 'LineString' as const, coordinates: [from, to] },
          properties: { isJump: leg.is_jump, segmentColor: leg.color },
        },
      ];
    });
    geojsonSource.setData({ type: 'FeatureCollection', features });
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
    const colorProp: ExpressionSpecification = ['get', 'color'] as ExpressionSpecification;
    const shotColor = this.distanceMode
      ? distanceColorExpression(
          DISTANCE_NEAR,
          DISTANCE_FAR,
          this.distanceState?.max ?? 0,
          this.themeColors.grayLine,
          ['get', 'dist'] as ExpressionSpecification,
        )
      : this.gasReachMode
        ? gasReachColorExpression(
            GAS_NEAR,
            GAS_FAR,
            this.gasReachState?.max ?? 0,
            this.themeColors.grayLine,
            ['get', 'gasReach'] as ExpressionSpecification,
          )
        : colorProp;
    if (this.map.getLayer(this.interactShotsLayerId)) {
      this.map.setPaintProperty(this.interactShotsLayerId, 'line-color', shotColor);
    }
    if (this.map.getLayer(this.interactStationsLayerId)) {
      this.map.setPaintProperty(this.interactStationsLayerId, 'circle-color', shotColor);
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

  /**
   * (Re)build the dedicated custom measure-path layer (#26, stage 2a) from the
   * current measured id sets and geometry index, or clear it when no path is
   * set. Called from setMeasuredPath and after each geometry rebuild (endApply-
   * Features re-applies the tint), mirroring applyMeasuredPathFeatureState so a
   * mid-measurement edit keeps the custom tint in sync with moved coords.
   *
   * The measured id sets are the encoded feature ids setMeasuredPath received
   * (`sectionId * FEATURE_ID_STRIDE + scopedId`); they are decoded back and
   * projected via the SoA geometry index (`shotGeometry` / `stationCoords`), the
   * same source the interact overlay uses. Reference-hidden features are skipped
   * to match the stock `measurePathFilter` / interact-overlay guards.
   *
   * z-order: inserted beneath `survey-interact-shots-measured` (the hover/
   * selection tint), which sits above the stock `survey-measure-path`. Both the
   * custom and stock tints therefore draw between the survey geometry and the
   * hover overlay in identical MEASURE_TINT - the pixels match this stage.
   */
  private syncCustomMeasurePath(): void {
    if (!this.measurePath) return;
    if (!this.map || !hasLiveStyle(this.map) || !this.geoIndex) {
      this.measurePath.clear();
      return;
    }
    if (this.measuredPathIds === null && this.measuredPathStationIds === null) {
      this.measurePath.clear();
      return;
    }
    // The tint must not float over hidden geometry: measured shots follow the
    // shots toggle, measured stations follow the stations toggle (setShowShots /
    // setShowStations re-run this so a live toggle updates the path).
    const segments: MeasureSegment[] = [];
    if (this.measuredPathIds && this.shotsVisible) {
      // Resolve by RE-DERIVING each shot's canonical feature id rather than
      // decoding the fid: the scoped id folded into a shot fid is the GLOBAL shot
      // id (>= FEATURE_ID_STRIDE for section >= 1), so a floor(fid / STRIDE) split
      // mis-attributes every such shot and drops its tint (#26). Hidden reference
      // legs are excluded to match the interact overlay / stock filter.
      for (const { from, to } of this.geoIndex.measuredShotSegments(
        this.measuredPathIds,
        this.visibleKinds(),
      )) {
        segments.push({ a: from, b: to });
      }
    }
    const points: MeasurePoint[] = [];
    if (this.measuredPathStationIds && this.stationsVisible) {
      for (const fid of this.measuredPathStationIds) {
        const sectionId = Math.floor(fid / FEATURE_ID_STRIDE);
        const stationIndex = fid - sectionId * FEATURE_ID_STRIDE;
        const ref = { section_id: sectionId, station_index: stationIndex };
        // Match the interact-overlay guard: a station hidden because every
        // kind touching it is hidden must not surface.
        if (!this.geoIndex.isStationDrawn(ref, this.visibleKinds()))
          continue;
        const coords = this.geoIndex.stationCoords(ref);
        if (coords) points.push({ center: coords });
      }
    }
    if (segments.length === 0 && points.length === 0) {
      this.measurePath.clear();
      return;
    }
    this.measurePath.setPath(this.buildMeasurePathRenderData(segments, points), [
      // ABOVE the custom survey geometry (which anchors before survey-interact-shots)
      // so the measured shot lines tint OVER the base geometry rather than being
      // covered by it; still below the interact hover/selection overlay. Added
      // after the geometry layer, so `before survey-interact-shots` lands it just
      // above the geometry. Fallback keeps it placed if that layer is absent.
      this.interactShotsLayerId,
      this.interactShotsMeasuredLayerId,
    ]);
  }

  /** Tessellate the measured path + resolve its device-px sizes and tint color
   *  for the custom measure-path layer. */
  private buildMeasurePathRenderData(
    segments: MeasureSegment[],
    points: MeasurePoint[],
  ): MeasurePathRenderData {
    const dpr = (typeof window !== 'undefined' && window.devicePixelRatio) || 1;
    const [r, g, b, a] = packCssColor(this.measureTintColor);
    const { anchorX, anchorY, line, circle } = buildMeasurePathBuffers(segments, points);
    return {
      anchorX,
      anchorY,
      line,
      circle,
      // Match the stock tint's line width (baseWidth = getShotWidth) and the
      // station disc radius (getStationRadius); half-width for the capsule.
      lineHalfWidthPx: (this.getShotWidth() / 2) * dpr,
      circleRadiusPx: this.getStationRadius() * dpr,
      aaPx: dpr,
      color: [r / 255, g / 255, b / 255, a / 255],
    };
  }

  // Update ghost hitbox layer sizes from config. The main shots/stations hit
  // areas are no longer layer paint (the dedicated hitbox layers were removed,
  // #mem budget); they are read live per query in hitCandidateSections via
  // getShot/StationHitReach(), so config changes take effect with no layer edit.
  private updateHitboxLayers(): void {
    if (!this.map) return;

    const stationHitArea = this.getStationHitArea();
    const shotHitArea = this.getShotHitArea();

    if (this.map.getLayer(this.ghostStationsHitboxLayerId)) {
      this.map.setPaintProperty(this.ghostStationsHitboxLayerId, 'circle-radius', stationHitArea);
    }
    if (this.map.getLayer(this.ghostShotsHitboxLayerId)) {
      this.map.setPaintProperty(this.ghostShotsHitboxLayerId, 'line-width', shotHitArea);
    }
  }

  // Update ghost visual layer sizes from config (merge preview layers)
  private updateGhostVisualLayers(): void {
    if (!this.map) return;

    const baseRadius = this.getStationRadius();
    const baseWidth = this.getShotWidth();

    // Update ghost visual layers to match config (guard layer existence)
    if (this.map.getLayer(this.ghostStationsLayerId)) {
      this.map.setPaintProperty(this.ghostStationsLayerId, 'circle-radius', baseRadius);
    }
    if (this.map.getLayer(this.ghostShotsLayerId)) {
      this.map.setPaintProperty(this.ghostShotsLayerId, 'line-width', baseWidth);
    }
  }

  // Update debug visualization based on config.
  // The main shots/stations hitbox layers were removed (#mem budget), so the
  // show_hitboxes debug toggle no longer paints a visible outline for the main
  // hit areas (hit-testing is a padded query on the visible layers now). The
  // ghost hitbox layers still honor it for merge-preview debugging.
  private updateDebugVisualization(): void {
    if (!this.map) return;

    const showHitboxes = this.config?.debug?.map?.show_hitboxes ?? false;
    const debugColor = 'rgba(255, 0, 0, 0.3)'; // Semi-transparent red for debug
    const opaqueColor = '#000000'; // OPAQUE color for normal mode (opacity controls visibility)

    // Ghost hitbox layers - debug visualization only (NOT used for hit detection)
    // Keep at opacity 0 when debug off to avoid unnecessary rendering work
    if (this.map.getLayer(this.ghostStationsHitboxLayerId)) {
      this.map.setPaintProperty(
        this.ghostStationsHitboxLayerId,
        'circle-color',
        showHitboxes ? debugColor : opaqueColor
      );
      this.map.setPaintProperty(
        this.ghostStationsHitboxLayerId,
        'circle-opacity',
        showHitboxes ? 1 : 0 // 0 when off - not used for hit detection
      );
    }

    if (this.map.getLayer(this.ghostShotsHitboxLayerId)) {
      this.map.setPaintProperty(
        this.ghostShotsHitboxLayerId,
        'line-color',
        showHitboxes ? debugColor : opaqueColor
      );
      this.map.setPaintProperty(
        this.ghostShotsHitboxLayerId,
        'line-opacity',
        showHitboxes ? 1 : 0 // 0 when off - not used for hit detection
      );
    }
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
    if (this.dragPreviewActive && !this.dragCommitPending) return;

    // Snap ring renders from the interact overlay (#mem): pushing one small
    // feature is as instant as the old per-feature feature-state was.
    this.refreshInteractOverlay();
  }

  /**
   * Query the nearest station to a screen point. The padded visible-layer query
   * (MapLibre's spatial index) gates the candidate sections; the SoA resolver
   * picks the exact station with a circular `threshold` (the old post-check
   * for corner candidates is inherent in the resolver's maxDistance) (#mem).
   */
  queryNearestStation(
    screenX: number,
    screenY: number,
    threshold: number
  ): StationRef | null {
    const bbox: [[number, number], [number, number]] = [
      [screenX - threshold, screenY - threshold],
      [screenX + threshold, screenY + threshold],
    ];
    const { stationSections } = this.hitCandidateSections(bbox);
    const nearest = this.resolveStationAt(screenX, screenY, stationSections, threshold);
    return nearest ? nearest.ref : null;
  }

  /**
   * Nearest shot to a screen point, plus the fractional position `t` ∈ [0,1]
   * along that shot's line (#26 ruler snap-to-shot). The caller interpolates the
   * 3D coords at `t` from the active stations (the 2D shot geometry has no z).
   * Returns null when no shot is within `threshold` pixels (circular - the
   * old corner post-check is inherent in the resolver's maxDistance).
   *
   * The SoA resolver computes `t` and the snap point from the TRUE shot
   * endpoints (#mem), which also fixes the previously-documented edge case
   * where tile-clipped rendered geometry skewed `t` for long shots.
   */
  queryNearestShotPoint(
    screenX: number,
    screenY: number,
    threshold: number
  ): { section_id: number; shot_id: number; t: number; lng: number; lat: number } | null {
    const bbox: [[number, number], [number, number]] = [
      [screenX - threshold, screenY - threshold],
      [screenX + threshold, screenY + threshold],
    ];
    const { shotSections } = this.hitCandidateSections(bbox);
    const nearest = this.resolveShotAt(screenX, screenY, shotSections, threshold);
    if (!nearest) return null;

    const lng = nearest.from[0] + nearest.t * (nearest.to[0] - nearest.from[0]);
    const lat = nearest.from[1] + nearest.t * (nearest.to[1] - nearest.from[1]);

    return {
      section_id: nearest.sectionId,
      shot_id: nearest.shotId,
      t: nearest.t,
      lng,
      lat,
    };
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
    const visibility = visible ? 'visible' : 'none';
    // Clear hover state FIRST when hiding to prevent any race conditions.
    if (!visible) {
      this.clearStationHover();
    }
    this.customRender?.setStationsVisible(visible);
    this.map.setLayoutProperty(this.loopClosuresLayerId, 'visibility', visibility);
    // Interact overlay tracks the stations layer: a hover/selection highlight
    // must not surface a hidden station (#mem).
    this.map.setLayoutProperty(this.interactStationsLayerId, 'visibility', visibility);
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
    const visibility = visible ? 'visible' : 'none';
    // Clear hover state FIRST when hiding to prevent any race conditions.
    if (!visible) {
      this.clearShotHover();
    }
    this.customRender?.setShotsVisible(visible);
    // Interact overlay tracks the shots layers (#mem).
    this.map.setLayoutProperty(this.interactShotsLayerId, 'visibility', visibility);
    this.map.setLayoutProperty(this.interactShotsMeasuredLayerId, 'visibility', visibility);
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
    this.updateVisualState();
  }

  /**
   * Set anchored section IDs for fading when Move Segment tool is active.
   * Sections in this set get reduced opacity via feature-state.
   */
  setAnchoredSections(sectionIds: Set<number>): void {
    const before = this.lastOverlayDigest;
    this.anchoredSectionIds = sectionIds;
    // Update loop closure line opacity - gray out closures where both endpoints are anchored.
    this.updateClosureAnchoredOpacity(sectionIds);
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
    const dim = buildCustomDimState(this.anchoredSectionIds, this.immovableStationFeatureIds, this.immovableShotFeatureIds, this.gasReachMode, this.gasReachState, this.isPlanIdleActive());
    this.customRender?.setDimming(dim);
    this.customLabels?.setDimming(dim);
  }

  // Set line-opacity on loop closures layer: 0.25 when both endpoints are in anchored sections
  private updateClosureAnchoredOpacity(anchoredIds: Set<number>): void {
    const idsArray = [...anchoredIds];

    if (idsArray.length > 0) {
      // Both endpoints must be anchored to gray out the closure line
      const bothAnchored: maplibregl.ExpressionSpecification = ['all',
        ['in', ['get', 'sectionIdA'], ['literal', idsArray]],
        ['in', ['get', 'sectionIdB'], ['literal', idsArray]],
      ] as maplibregl.ExpressionSpecification;

      this.map.setPaintProperty(this.loopClosuresLayerId, 'line-opacity', [
        'case', bothAnchored, 0.25, 0.8,
      ] as maplibregl.ExpressionSpecification);
    } else {
      this.map.setPaintProperty(this.loopClosuresLayerId, 'line-opacity', 0.8);
    }
  }

  /**
   * Set individual feature-level immovable state for move_station / move_shot tools.
   * Uses setPaintProperty with data-driven expressions to dim immovable features.
   * Unlike setAnchoredSections (which re-tessellates dim flags), this approach is reliable
   * on initial tool activation because it doesn't depend on feature tiling state.
   */
  setImmovableFeatures(stationIds: Set<number>, shotIds: Set<number>): void {
    const before = this.lastOverlayDigest;
    this.immovableStationFeatureIds = stationIds;
    this.immovableShotFeatureIds = shotIds;
    // Loop closures dim if either endpoint is immovable (kept GeoJSON overlay).
    this.updateImmovableClosureOpacity(stationIds);
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

  private updateImmovableClosureOpacity(immovableStationIds: Set<number>): void {
    if (immovableStationIds.size > 0) {
      // Build station key strings matching closure feature "sectionId:stationIndex" format
      const immovableKeyStrings: string[] = [];
      for (const fid of immovableStationIds) {
        const sectionId = Math.floor(fid / FEATURE_ID_STRIDE);
        const stationIndex = fid % FEATURE_ID_STRIDE;
        immovableKeyStrings.push(`${sectionId}:${stationIndex}`);
      }

      // Dim closure if either endpoint is immovable
      const eitherImmovable: maplibregl.ExpressionSpecification = ['any',
        ['in', ['get', 'stationA'], ['literal', immovableKeyStrings]],
        ['in', ['get', 'stationB'], ['literal', immovableKeyStrings]],
      ] as maplibregl.ExpressionSpecification;

      this.map.setPaintProperty(this.loopClosuresLayerId, 'line-opacity', [
        'case', eitherImmovable, 0.25, 0.8,
      ] as maplibregl.ExpressionSpecification);
    } else {
      this.map.setPaintProperty(this.loopClosuresLayerId, 'line-opacity', 0.8);
    }
  }

  /** The drag preview's shot/station color paint. Plain mode reads the resolved
   *  per-feature `color` property (baked at capture time, like the interact
   *  overlay); distance / gas-reach modes read the carried `dist` / `gasReach`
   *  through the SAME ramp expressions the bulk layers use, so colors match. */
  private dragPreviewColorExpr(): ExpressionSpecification {
    if (this.distanceMode) {
      return distanceColorExpression(
        DISTANCE_NEAR,
        DISTANCE_FAR,
        this.distanceState?.max ?? 0,
        this.themeColors.grayLine,
        ['get', 'dist'] as ExpressionSpecification,
      );
    }
    if (this.gasReachMode) {
      return gasReachColorExpression(
        GAS_NEAR,
        GAS_FAR,
        this.gasReachState?.max ?? 0,
        this.themeColors.grayLine,
        ['get', 'gasReach'] as ExpressionSpecification,
      );
    }
    return ['get', 'color'] as ExpressionSpecification;
  }

  /** Build the layer-spec context for the drag-preview layers module: shots +
   *  stations color / size / highlight (see surveyLayer/dragPreviewLayers.ts). */
  private dragPreviewLayerCtx(): DragPreviewLayerCtx {
    return {
      colorExpr: this.dragPreviewColorExpr(),
      shotWidth: this.getShotSelectionWidth(),
      stationRadius: this.getStationSelectionRadius(),
      selectHighlight: this.themeColors.selectHighlight,
    };
  }

  /** Install the captured drag preview geometry (shots / stations / closures) as
   *  the moving ghost, and hide the moving originals. Attached text is not
   *  previewed - it is hidden from the stock text layers for the drag duration by
   *  the begin methods. Shared by the segment and station/shot begins.
   *
   *  The ghost is a custom WebGL layer whose live drag delta is a single
   *  vertex-shader uniform (no worker, no re-bake -> no overshoot). It draws
   *  synchronously on the next frame, so the moving originals are hidden NOW (they
   *  overlap the ghost at offset 0). If the custom ghost is not the active path
   *  (bulk geometry stock, or the ghost layer failed / self-disabled), the STOCK
   *  GeoJSON preview stands in and its async setData means the hide waits for the
   *  first source load (START-flash fix). See {@link DragGhostController}. */
  private installDragPreview(features: DragPreviewFeature[], exclusion: CustomRenderExclusion): void {
    // New drag generation: invalidates any still-pending deferred drag-hide from
    // a superseded drag (guards the sourcedata callback in
    // setDragExclusionWhenPreviewReady).
    this.dragGeneration += 1;
    this.dragPreviewSet = { features };
    this.dragPreviewActive = true;
    if (this.dragGhost.begin(this.dragPreviewSet) === 'custom') this.setDragExclusion(exclusion);
    else this.setDragExclusionWhenPreviewReady(exclusion);
    this.renderOverlayFollow(0, 0);
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
      overlay: overlayInputFrom(this.distanceMode, this.distanceState, this.gasReachMode, this.gasReachState),
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
      movingEndForShot: (id) => this.dragMovingShots.get(id),
      isMovingStation: (id) => this.dragMovingStationKeys.has(id),
      parseFid: (id) => this.parseFeatureId(id),
      isAnchored: (id) => this.isSectionAnchored(sectionIdFromKey(id)),
      isShotImmovable: (fid) => this.isShotImmovable(fid),
      isStationImmovable: (fid) => this.isStationImmovable(fid),
      shotFeature: (id, w) => this.shotOverlayFeature(id, w),
      stationFeature: (id, s) => this.stationOverlayFeature(id, s),
    });
  }

  /** Re-emit any hover/selected highlight on a moving feature at the drag offset
   *  so it tracks the cursor instead of sticking at the committed position (#C).
   *  A small setData, coalesced one-per-frame by the rAF (see scheduleDragRender).
   *  Kept separate from the ghost bake so the highlight stays as responsive as
   *  before while the ghost bakes single-flight. */
  private renderOverlayFollow(dx: number, dy: number): void {
    if (!this.dragPreviewActive) return;
    this.dragOverlayOffset = { dx, dy };
    this.refreshInteractOverlay();
  }

  /** The map-backed projector used for the preview + overlay offset transforms. */
  private dragProjector(): Projector {
    return {
      project: (lngLat) => this.map.project(lngLat),
      unproject: (pt) => this.map.unproject(pt),
    };
  }

  /** Stop the interact overlay following the drag and snap it back to committed
   *  coordinates (called on commit / end). */
  private clearDragOverlayFollow(): void {
    this.dragOverlayOffset = null;
    this.dragCommitPending = false;
    this.dragMovingStationKeys.clear();
    this.dragMovingShots.clear();
    this.lastInteractKey = null;
    this.refreshInteractOverlay();
  }

  /** The per-feature analytical-overlay props (dist / gas-reach) a moving feature
   *  must carry so the preview's ramp paint matches the bulk layers. Keyed the
   *  same way the interact overlay keys them ("sec:idx" / "sec:shotId"). */
  private dragOverlayProps(idStr: string, kind: 'station' | 'shot'): Record<string, unknown> {
    const props: Record<string, unknown> = {};
    if (!this.distanceMode && !this.gasReachMode) return props;
    const fid = this.parseFeatureId(idStr);
    if (fid === null) return props;
    const map = kind === 'station'
      ? { dist: this.distanceState?.stations, gas: this.gasReachState?.stations }
      : { dist: this.distanceState?.shots, gas: this.gasReachState?.shots };
    if (this.distanceMode) props.dist = map.dist?.get(fid) ?? -1;
    if (this.gasReachMode) props.gasReach = map.gas?.get(fid) ?? -1;
    return props;
  }

  /**
   * Begin SEGMENT drag preview - hide the connected sections' real features from
   * the bulk layers (excluded by sectionId), capture every moving station / shot
   * / label / closure ONCE with its original coordinates, and render the moving
   * copy from the unified preview source. All geometry moves uniformly, so a
   * later per-frame offset is applied to every captured vertex.
   */
  beginDragPreview(connectedSectionIds: Set<number>): void {
    if (this.dragPreviewActive) this.endDragPreview();
    if (connectedSectionIds.size === 0) return;
    if (!this.geoIndex || !this.cachedSurveyData) return;

    const idsArray = [...connectedSectionIds];

    // Hide closures with EITHER endpoint in the connected set. Closures with BOTH
    // endpoints inside move uniformly (captured below); closures bridging a
    // dragged and a stationary section are hidden during the drag and reappear
    // with correct geometry on commit. (The survey geometry + labels are hidden in
    // the custom layers via the drag exclusion in installDragPreview.)
    const existingClosureFilter = this.map.getFilter(this.loopClosuresLayerId);
    this.dragSavedFilters.set(this.loopClosuresLayerId, existingClosureFilter ?? undefined);
    const eitherEndpointExclusion: maplibregl.FilterSpecification = ['!', ['any',
      ['in', ['get', 'sectionIdA'], ['literal', idsArray]],
      ['in', ['get', 'sectionIdB'], ['literal', idsArray]],
    ]] as unknown as maplibregl.FilterSpecification;
    this.map.setFilter(this.loopClosuresLayerId, (existingClosureFilter && typeof existingClosureFilter === 'object')
      ? (['all', existingClosureFilter, eitherEndpointExclusion] as unknown as maplibregl.FilterSpecification)
      : eitherEndpointExclusion);

    // Capture the moving geometry once. Everything in the segment moves uniformly.
    // Attached text (labels / comments / section names) is not captured - it is
    // hidden from the stock layers above, not previewed.
    const features: DragPreviewFeature[] = [];
    const ctx = this.displayColorContext();
    const stationDefault = this.themeColors.stationDefault;

    // Read the moving geometry from the SoA (sectionGeometry), NOT from the lazy
    // `Station[]` proxy or the lazy `section.shots` getter: filtering all ~89k
    // stations per section (and a linear `shotGeometry` scan per shot) made this
    // quadratic and froze a 14k-section project on drag start (#mem).
    // Iterate cachedSections (not the Set) to keep the original emit order; reading
    // `id`/`color` off a section does not trip its lazy `shots` getter.
    for (const sectionMeta of this.cachedSections) {
      const sectionId = sectionMeta.id;
      if (!connectedSectionIds.has(sectionId)) continue;
      const sectionColor = sectionMeta.color ?? stationDefault;
      const { stations: sectionStations, shots: sectionShots } =
        this.geoIndex.sectionGeometry(sectionId);

      // Stations (points).
      for (const st of sectionStations) {
        const idStr = `${sectionId}:${st.stationIndex}`;
        this.dragMovingStationKeys.add(idStr);
        const color = resolveStationDisplayColor(
          this.cachedSurveyData,
          sectionColor,
          st.globalIdx,
          ctx,
        );
        features.push({
          emit: 'point',
          coords: [st.lngLat],
          movingEnd: 'uniform',
          properties: { kind: 'station', color, ...this.dragOverlayProps(idStr, 'station') },
        });
      }

      // Shots (lines).
      for (const shot of sectionShots) {
        const shotKey = `${sectionId}:${shot.shotId}`;
        const color = resolveShotDisplayColor(
          this.cachedSurveyData,
          sectionColor,
          shot.globalIdx,
          ctx,
        );
        this.dragMovingShots.set(shotKey, 'both');
        features.push({
          emit: 'line',
          coords: [shot.from, shot.to],
          movingEnd: 'uniform',
          properties: { kind: 'shot', color, ...this.dragOverlayProps(shotKey, 'shot') },
        });
      }
    }

    // Loop closures with BOTH endpoints in the moving set (move uniformly).
    for (const lc of this.cachedLoopClosures ?? []) {
      if (!connectedSectionIds.has(lc.station_a.section_id) || !connectedSectionIds.has(lc.station_b.section_id)) continue;
      const a = this.geoIndex.stationCoords(lc.station_a);
      const b = this.geoIndex.stationCoords(lc.station_b);
      if (!a || !b) continue;
      features.push({
        emit: 'line',
        coords: [a, b],
        movingEnd: 'uniform',
        properties: { kind: 'closure', color: getErrorColor(lc.error_percentage) },
      });
    }

    // Segment drag hides whole connected sections (shots + stations) in the
    // custom layer, mirroring the sectionId exclusion applied to the stock layers.
    this.installDragPreview(features, { sectionIds: new Set(idsArray) });
  }

  /**
   * Begin station / shot drag preview - finds all co-located stations
   * (merged/closured), hides them and their connected shots/labels/closures from
   * the bulk layers, captures that moving geometry (with per-shot movingEnd so a
   * shot stretches around its fixed endpoint), and renders the moving copy from
   * the unified preview source. Everything moves through ONE per-frame setData.
   *
   * Returns the list of co-located StationRefs so MapCanvas can batch-move them.
   */
  beginStationDragPreview(sectionId: number, stationIndex: number, additionalSeeds?: StationRef[]): StationRef[] {
    if (this.dragPreviewActive) this.endDragPreview();

    // 1. Find clicked station and all co-located stations via BFS:
    //    - merged_with: always co-located
    //    - closures_with: only when the closure has been aligned
    //    additionalSeeds allows shot drag to seed from both endpoints simultaneously
    // key -> global SoA index, built from NUMBERS only. Mapping the lazy
    // `Station[]` proxy here materialized the whole ~89k station object graph on
    // every drag start (and `.filter()` below did it a second time) - the same
    // quadratic trap that froze segment drags (#mem).
    const data = this.cachedSurveyData;
    if (!data) return [];
    const globalByKey = new Map<string, number>();
    for (let g = 0; g < data.stationCount; g++) {
      globalByKey.set(`${data.stationSectionIds[g]}:${data.stationIndices[g]}`, g);
    }
    const alignedClosureIds = new Set<bigint>(
      (this.cachedLoopClosures ?? [])
        .filter((closure) => closure.is_aligned)
        .map((closure) => closure.id)
    );
    const clickedKey = `${sectionId}:${stationIndex}`;
    if (!globalByKey.has(clickedKey)) return [];

    // Seed BFS from primary station + any additional seeds (e.g., shot's other endpoint)
    const seedKeys = [clickedKey];
    if (additionalSeeds) {
      for (const seed of additionalSeeds) {
        const key = `${seed.section_id}:${seed.station_index}`;
        if (globalByKey.has(key)) seedKeys.push(key);
      }
    }

    const movingRefKeys = new Set<string>();
    const queue: string[] = [...seedKeys];
    while (queue.length > 0) {
      const key = queue.pop()!;
      if (movingRefKeys.has(key)) continue;
      movingRefKeys.add(key);
      const g = globalByKey.get(key);
      if (g === undefined) continue;
      for (const ref of data.stationMergedWith.get(g) ?? []) {
        queue.push(`${ref.section_id}:${ref.station_index}`);
      }
      for (const cr of data.stationClosuresWith.get(g) ?? []) {
        if (!alignedClosureIds.has(cr.closure_id)) continue;
        queue.push(`${cr.station_ref.section_id}:${cr.station_ref.station_index}`);
      }
    }
    // The moving set is a merge/closure cluster (small); resolve each member's
    // coords + global index straight from the SoA.
    const movingStations = [...movingRefKeys].flatMap((key) => {
      const g = globalByKey.get(key);
      if (g === undefined) return [];
      const base = g * 3;
      return [{
        station_ref: {
          section_id: data.stationSectionIds[g]!,
          station_index: data.stationIndices[g]!,
        } as StationRef,
        lngLat: metersToMapCoords(
          data.stationCoords[base]!,
          data.stationCoords[base + 1]!,
          this.gpsOrigin,
        ) as [number, number],
        globalIdx: g,
      }];
    });

    // 2. Capture the moving geometry once (originals + movingEnd). The moving copy
    // renders from the drag ghost; the originals are hidden in the custom layers via
    // the drag exclusion in installDragPreview. Only endpoints in the moving set
    // follow the offset, so a shot stretches around a fixed end instead of whole.
    if (!this.geoIndex || !this.cachedSurveyData) return movingStations.map(s => s.station_ref);
    const ctx = this.displayColorContext();
    const stationDefault = this.themeColors.stationDefault;
    const features: DragPreviewFeature[] = [];
    // Custom-render drag exclusion: the moving stations + shots, keyed by global
    // SoA index for O(1) skip in tessellate.
    const excludedStationGlobals = new Set<number>();
    const excludedShotGlobals = new Set<number>();
    // Sections owning any moved station / shot: their NAME label drag-hides too
    // (custom label layer), so it does not sit pinned at the old anchor while the
    // partial geometry moves. Whole-section segment drags carry the name via
    // `sectionIds` instead (see beginDragPreview).
    const affectedSectionIds = new Set<number>();

    // Moving stations (points) - all uniform.
    for (const st of movingStations) {
      const ref = st.station_ref;
      const idStr = `${ref.section_id}:${ref.station_index}`;
      this.dragMovingStationKeys.add(idStr);
      const section = this.geoIndex.section(ref.section_id);
      excludedStationGlobals.add(st.globalIdx);
      affectedSectionIds.add(ref.section_id);
      const color = resolveStationDisplayColor(
        this.cachedSurveyData,
        section?.color ?? stationDefault,
        st.globalIdx,
        ctx,
      );
      features.push({
        emit: 'point',
        coords: [st.lngLat],
        movingEnd: 'uniform',
        properties: { kind: 'station', color, ...this.dragOverlayProps(idStr, 'station') },
      });
    }

    // 4. Connected shots (lines) + shot labels. Walk each AFFECTED SECTION once
    // (from the SoA), not once per moving station: re-filtering the lazy
    // `Station[]` proxy per station materialized ~89k objects each time, and the
    // per-shot `shotGeometry()` scan made it quadratic in section size (#mem).
    const seenShotKeys = new Set<string>();
    const movingSectionIds = new Set(movingStations.map((st) => st.station_ref.section_id));
    for (const secId of movingSectionIds) {
      const section = this.geoIndex.section(secId);
      if (!section) continue;
      const { shots: sectionShots } = this.geoIndex.sectionGeometry(secId);

      for (const shot of sectionShots) {
        // Shot `index` joins station `index` to `index + 1`.
        const fromKey = `${secId}:${shot.index}`;
        const toKey = `${secId}:${shot.index + 1}`;
        const fromIsMoving = movingRefKeys.has(fromKey);
        const toIsMoving = movingRefKeys.has(toKey);
        if (!fromIsMoving && !toIsMoving) continue;

        const shotKey = `${secId}:${shot.shotId}`;
        if (seenShotKeys.has(shotKey)) continue;
        seenShotKeys.add(shotKey);

        const movingEnd: MovingEnd = fromIsMoving && toIsMoving ? 'both' : fromIsMoving ? 'from' : 'to';
        this.dragMovingShots.set(shotKey, movingEnd);
        const fromCoord = shot.from;
        const toCoord = shot.to;
        excludedShotGlobals.add(shot.globalIdx);
        affectedSectionIds.add(secId);
        const color = resolveShotDisplayColor(
          this.cachedSurveyData,
          section.color ?? stationDefault,
          shot.globalIdx,
          ctx,
        );
        features.push({
          emit: 'line',
          coords: [fromCoord, toCoord],
          movingEnd,
          properties: { kind: 'shot', color, ...this.dragOverlayProps(shotKey, 'shot') },
        });
      }
    }

    // (The moving shots are hidden in the custom geometry + label layers via the
    // drag exclusion in installDragPreview; no stock-layer filters to set.)

    // 5. Closure indicator lines with >= 1 endpoint in the moving set; hide those
    // from the bulk closures layer (they render in the preview, stretched).
    const closureExclusions: maplibregl.FilterSpecification[] = [];
    for (const lc of this.cachedLoopClosures ?? []) {
      const keyA = `${lc.station_a.section_id}:${lc.station_a.station_index}`;
      const keyB = `${lc.station_b.section_id}:${lc.station_b.station_index}`;
      const aMoving = movingRefKeys.has(keyA);
      const bMoving = movingRefKeys.has(keyB);
      if (!aMoving && !bMoving) continue;

      // Coords from the SoA (O(1) per endpoint) - no Station materialization.
      const fromCoord = this.geoIndex.stationCoords(lc.station_a);
      const toCoord = this.geoIndex.stationCoords(lc.station_b);
      if (!fromCoord || !toCoord) continue;
      features.push({
        emit: 'line',
        coords: [fromCoord, toCoord],
        movingEnd: aMoving && bMoving ? 'both' : aMoving ? 'from' : 'to',
        properties: { kind: 'closure', color: getErrorColor(lc.error_percentage) },
      });
      closureExclusions.push(['all',
        ['==', ['get', 'stationA'], keyA],
        ['==', ['get', 'stationB'], keyB],
      ] as unknown as maplibregl.FilterSpecification);
    }
    if (closureExclusions.length > 0) {
      const closureExclusion: maplibregl.FilterSpecification = ['!',
        ['any', ...closureExclusions],
      ] as unknown as maplibregl.FilterSpecification;
      const existingClosureFilter = this.map.getFilter(this.loopClosuresLayerId);
      this.dragSavedFilters.set(this.loopClosuresLayerId, existingClosureFilter ?? undefined);
      const composedClosure: maplibregl.FilterSpecification = (existingClosureFilter && typeof existingClosureFilter === 'object')
        ? (['all', existingClosureFilter, closureExclusion] as unknown as maplibregl.FilterSpecification)
        : closureExclusion;
      this.map.setFilter(this.loopClosuresLayerId, composedClosure);
    }

    // Hide the moving originals from the custom layer. installDragPreview applies
    // this now for the custom ghost (draws next frame) or defers it behind the
    // async stock preview's first load (START-flash fix).
    this.installDragPreview(features, {
      stationGlobalIndices: excludedStationGlobals,
      shotGlobalIndices: excludedShotGlobals,
      affectedSectionIds,
    });
    return movingStations.map(s => s.station_ref);
  }

  /** Point the custom-render layer at (or off, with null) the active drag's
   *  hidden feature set, so the originals vanish at drag begin and reappear at
   *  drag end - matching the stock bulk layers' setFilter exclusions the custom
   *  layer cannot see. Cheap: recomputes + re-uploads only the per-vertex hide
   *  flag (no buffer rebuild). No-op unless the custom render gate is on. */
  private setDragExclusion(exclusion: CustomRenderExclusion | null): void {
    this.customRender?.setDragHide(exclusion);
    // Labels mirror the SAME exclusion: the dragged features' labels collapse via
    // the DRAG_HIDE flag (synchronous GPU upload), replacing the stock text-layer
    // filter drop. On drag commit the synchronous sync() rebuild reflows them at
    // the new positions and setDragExclusion(null) clears the flag - labels never
    // lag the geometry (the anti-flicker payoff).
    this.customLabels?.setDragHide(exclusion);
  }

  /**
   * Apply the custom-render drag-hide only once the drag-preview source has
   * parsed and drawn its first frame (START-flash fix). setDragHide is a
   * synchronous GPU upload that takes effect on the very next frame, but the
   * preview's GeoJSON setData is async (worker parse + retile, ~5 frames on a
   * real project). Hiding the originals eagerly therefore blanks the geometry
   * for those frames - it is in neither the custom layer nor the not-yet-drawn
   * preview. Deferring the hide to the source's first `sourcedata`-loaded event
   * keeps the originals visible until the preview stands in for them (at offset
   * ~0 they overlap, so there is no visible double either), then swaps to the
   * preview atomically.
   */
  private setDragExclusionWhenPreviewReady(exclusion: CustomRenderExclusion): void {
    this.clearPendingDragHide();
    // If the source is already parsed (rare - typically false right after the
    // begin's setData), hide immediately; nothing to wait for.
    if (this.map.isSourceLoaded(DRAG_PREVIEW_SOURCE)) {
      this.setDragExclusion(exclusion);
      return;
    }
    const gen = this.dragGeneration;
    const listener = (e: maplibregl.MapSourceDataEvent): void => {
      if (e.sourceId !== DRAG_PREVIEW_SOURCE) return;
      if (!this.map.isSourceLoaded(DRAG_PREVIEW_SOURCE)) return;
      this.map.off('sourcedata', listener);
      if (this.pendingDragHide?.listener === listener) this.pendingDragHide = null;
      // Ignore a stale callback from a drag that has since ended or been replaced.
      if (gen !== this.dragGeneration || !this.dragPreviewActive) return;
      this.setDragExclusion(exclusion);
    };
    this.pendingDragHide = { exclusion, listener };
    this.map.on('sourcedata', listener);
  }

  /** Tear down a still-pending deferred drag-hide without applying it (on end). */
  private clearPendingDragHide(): void {
    if (!this.pendingDragHide) return;
    this.map.off('sourcedata', this.pendingDragHide.listener);
    this.pendingDragHide = null;
  }

  /** Force-apply a still-pending deferred drag-hide immediately (on commit). If a
   *  very fast drag commits before the async preview ever loaded, the originals
   *  must be hidden NOW so they cannot show through the preview that is kept alive
   *  across the IPC window (see commitDragPreview). No-op once already applied. */
  private flushPendingDragHide(): void {
    const pending = this.pendingDragHide;
    if (!pending) return;
    this.map.off('sourcedata', pending.listener);
    this.pendingDragHide = null;
    if (this.dragPreviewActive) this.setDragExclusion(pending.exclusion);
  }

  /** Drive both followers from a pointermove offset. The GHOST is driven
   *  SYNCHRONOUSLY by the controller (custom path: one shader-uniform write +
   *  triggerRepaint, no worker; stock fallback: the single-flight bake / translate
   *  machine). The interact overlay follow stays rAF-coalesced (one small setData
   *  per frame). */
  private scheduleDragRender(dx: number, dy: number): void {
    this.dragGhost.move(dx, dy);
    this.pendingDragOffset = { dx, dy };
    if (this.dragUpdateRaf !== null) return;
    this.dragUpdateRaf = requestAnimationFrame(() => {
      this.dragUpdateRaf = null;
      const off = this.pendingDragOffset;
      this.pendingDragOffset = null;
      if (off) this.renderOverlayFollow(off.dx, off.dy);
    });
  }

  /** Cancel any pending coalesced render (on commit / end). */
  private cancelDragRender(): void {
    if (this.dragUpdateRaf !== null) {
      cancelAnimationFrame(this.dragUpdateRaf);
      this.dragUpdateRaf = null;
    }
    this.pendingDragOffset = null;
  }

  /**
   * Update the station / shot drag preview - one setData over the unified source
   * (stations, shots, labels, closures all move together, no skew).
   */
  updateStationDragPreview(dx: number, dy: number): void {
    this.scheduleDragRender(dx, dy);
  }

  /**
   * Update the segment drag preview - one setData over the unified source. The
   * whole segment moves uniformly, so every captured vertex takes the offset.
   */
  updateDragPreview(dx: number, dy: number): void {
    this.scheduleDragRender(dx, dy);
  }

  /**
   * Commit drag preview - removes the preview layers but keeps the exclusion
   * filters on the bulk layers so the moved features stay hidden until React
   * re-renders with their new positions. Call this before the backend commit,
   * then endDragPreview() after the re-render. This prevents a blink where
   * features briefly appear at newPosition + dragOffset.
   */
  commitDragPreview(): void {
    if (!this.dragPreviewActive) return;
    this.cancelDragRender();
    // FINISH-flash fix: do NOT remove the preview here. cancelDragRender above
    // freezes it at the placement offset (its last setData), which is where the
    // committed geometry will land. The committed geometry does not exist
    // client-side until the post-IPC flush installs it (~100ms), and the
    // custom-render drag-hide stays applied until endDragPreview - so removing
    // the preview now would leave the moved geometry in NEITHER the preview nor
    // the custom layer for the whole IPC window: a blank hole where the user just
    // placed it. Instead endDragPreview (sequenced via onNextUpdate, firing in the
    // same synchronous flush block that installs the committed geometry and clears
    // the hide) removes the preview and swaps to the committed geometry atomically.
    // Force-apply any still-deferred hide now so a very fast drag's stale
    // originals cannot show through the kept preview during the window.
    this.flushPendingDragHide();
    // Keep the interact overlay following (dragOverlayOffset + moving sets) across
    // the IPC + flush window so the grabbed highlight stays glued to the cursor
    // instead of snapping back to the OLD coords for the ~100ms until the committed
    // geometry lands. endApplyFeatures drops the offset atomically, right before the
    // post-commit overlay refresh reads the installed geometry - so the overlay
    // never renders old+offset after the install (no 2x flash) nor plain old coords
    // during the window (no snap-back). It must survive until the geometry swaps,
    // hence the dragCommitPending handoff. (dragPreviewSet is nulled by the
    // endDragPreview that MapCanvas sequences right after this via onNextUpdate.)
    this.dragCommitPending = true;
  }

  /**
   * End drag preview - removes the preview layers (if still present) and restores
   * every saved bulk-layer filter so the real features are visible again.
   */
  endDragPreview(): void {
    if (!this.dragPreviewActive) return;

    this.cancelDragRender();
    // Tear down the ghost (custom layer or stock follow machine + preview layers).
    // This is the single removal point for the preview now that commitDragPreview
    // keeps it alive across the IPC window.
    this.dragGhost.end();
    // Drop any deferred drag-hide that never fired (drag ended before ready).
    this.clearPendingDragHide();
    // Restore the saved auxiliary filters (only loop closures now - the survey
    // geometry + labels are custom-rendered and their drag exclusion is dropped
    // synchronously below, so they land at the new position at once).
    for (const [layerId, saved] of this.dragSavedFilters) {
      if (this.map.getLayer(layerId)) this.map.setFilter(layerId, saved ?? null);
    }
    this.clearDragOverlayFollow();

    this.dragPreviewSet = null;
    this.dragSavedFilters.clear();
    this.dragPreviewActive = false;
    // Drop the custom-render exclusion and rebuild full buffers so the moved
    // features reappear at their committed positions (mirrors the filter restore).
    this.setDragExclusion(null);
  }

  /**
   * Set ghost section data for merge preview visualization.
   * Pass null to clear the ghost section.
   */
  setGhostSection(data: GhostSectionData | null) {
    const { shotFeatures, stationFeatures, mergeStationFeatures } = buildGhostFeatures(
      data,
      this.gpsOrigin
    );

    // Update sources
    const ghostShotsSource = this.map.getSource(this.sourceId + '-ghost-shots');
    if (ghostShotsSource && 'setData' in ghostShotsSource) {
      (ghostShotsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: shotFeatures,
      });
    }

    const ghostStationsSource = this.map.getSource(this.sourceId + '-ghost-stations');
    if (ghostStationsSource && 'setData' in ghostStationsSource) {
      (ghostStationsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: stationFeatures,
      });
    }

    const ghostMergeSource = this.map.getSource(this.sourceId + '-ghost-merge');
    if (ghostMergeSource && 'setData' in ghostMergeSource) {
      (ghostMergeSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: mergeStationFeatures,
      });
    }
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
      overlay: overlayInputFrom(this.distanceMode, this.distanceState, this.gasReachMode, this.gasReachState),
      // Move-comment palette tint (#52): custom features carry no commentColor
      // property, so the resolvers recompute it (only while move_comment is active).
      commentTint: this.activeTool === 'move_comment',
      // Plan-mode idle tint (#395 revision): paints every feature the gas ramp's
      // dim far color while there is no origin yet.
      planIdleTint: this.isPlanIdleActive(),
    };
    this.customRender.sync({
      surveyData,
      gpsOrigin: this.gpsOrigin,
      colorCtx,
      shotWidthPx: this.getShotWidth(),
      stationRadiusPx: this.getStationRadius(),
      stationStrokePx: SurveyLayer.STATION_STROKE_PX,
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
    if (isStyleRestoring(this.map)) {
      this.deferredUpdate = () =>
        this.flushUpdate(
          sections, stations, loopClosures, gpsOrigin, showDepthGradient, depthRange,
          precision, showIssueSeverity, warnings,
        );
      return;
    }
    // Early bail when the map is gone or its style has been torn down - the
    // coalescer's queueMicrotask can race with MapCanvas unmount → map.remove().
    if (!hasLiveStyle(this.map)) return;

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
    this.publishLoopClosures();

    // Delete straggler: a selected / hovered / snapped feature this flush removed
    // must drop from the interact overlay now (its custom geometry already vanished
    // above), instead of lingering. Skipped mid-drag (the drag paths own the overlay
    // follow). Hide the interact layers across the overlay GeoJSON source's own async
    // re-tile so the stale highlight drops in the geometry frame.
    if (
      !this.dragPreviewActive &&
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
    if (this.dragCommitPending) this.clearDragOverlayFollow();
    this.updateVisualState();
    // Gas route highlight (#233): re-push the route (coords come from cachedStations).
    if (this.gasRouteLegs.length > 0) this.applyGasRoute();

    // Fire the one-shot callback (drag commit restores its filters in the same
    // synchronous block that installed the geometry, so there is no paint gap).
    if (this.onNextUpdateCallback) {
      const cb = this.onNextUpdateCallback;
      this.onNextUpdateCallback = null;
      cb();
    }
  }

  /** Publish the loop-closure indicator lines to their GeoJSON source from the
   *  cached closures + the geometry index. The custom renderer draws everything
   *  else; loop closures stay a small stock GeoJSON overlay, like the gas route. */
  private publishLoopClosures(): void {
    const source = this.map.getSource(this.sourceId + '-loop-closures');
    if (!source || source.type !== 'geojson' || !this.geoIndex) return;
    const features: GeoJSON.Feature[] = [];
    for (const lc of this.cachedLoopClosures ?? []) {
      const a = this.geoIndex.stationCoords(lc.station_a);
      const b = this.geoIndex.stationCoords(lc.station_b);
      if (!a || !b) continue;
      features.push({
        type: 'Feature',
        geometry: { type: 'LineString', coordinates: [a, b] },
        properties: {
          stationA: `${lc.station_a.section_id}:${lc.station_a.station_index}`,
          stationB: `${lc.station_b.section_id}:${lc.station_b.station_index}`,
          sectionIdA: lc.station_a.section_id,
          sectionIdB: lc.station_b.section_id,
          errorPercentage: lc.error_percentage,
          misclosureDistance: lc.misclosure_distance,
          pathLength: lc.path_length,
          color: getErrorColor(lc.error_percentage),
        },
      });
    }
    (source as maplibregl.GeoJSONSource).setData({ type: 'FeatureCollection', features });
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
    // Ghost station stroke
    if (this.map.getLayer(this.ghostStationsLayerId)) {
      this.map.setPaintProperty(this.ghostStationsLayerId, 'circle-stroke-color', colors.strokeColor);
    }
    // Ghost merge station
    if (this.map.getLayer(this.ghostMergeLayerId)) {
      this.map.setPaintProperty(this.ghostMergeLayerId, 'circle-color', colors.selectHighlight);
      this.map.setPaintProperty(this.ghostMergeLayerId, 'circle-stroke-color', colors.strokeColor);
    }
    // Station highlight (snap color)
    if (this.map.getLayer(this.stationsHighlightLayerId)) {
      this.map.setPaintProperty(this.stationsHighlightLayerId, 'circle-color', colors.snapColor);
      this.map.setPaintProperty(this.stationsHighlightLayerId, 'circle-stroke-color', colors.snapColor);
    }

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

  /** Dispose the custom-render controllers WITHOUT touching the map (no
   *  removeLayer). Called on MapCanvas unmount, which deliberately skips the full
   *  {@link destroy} for unmount speed. Releases the controllers' window sync
   *  hooks so they no longer pin this layer's cached SurveyData + glyph atlas + GL
   *  buffers alive until the next SurveyLayer overwrites the globals (#mem).
   *  dispose() is the map-free counterpart of destroy(); it is idempotent, so the
   *  destroy() path (which nulls both refs before calling this) is unaffected. */
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
    const off = this.dragOverlayOffset;
    if (!off) return null;
    return { mode: this.dragGhost.isCustom() ? 'custom' : 'stock', dx: off.dx, dy: off.dy };
  }

  releaseVtRegistry(): void {
    this.customRender?.dispose();
    this.customLabels?.dispose();
    // The DPR watcher's matchMedia listener is a JS resource that survives
    // map.remove() and would otherwise retain this whole SurveyLayer (cached
    // survey data + geometry index). MapCanvas's lightweight unmount calls this
    // method but NOT destroy(), so the disposal must live here to run on both
    // paths - else repeated project opens accumulate stale layers (#customRender).
    this.dprWatcherDispose?.();
    this.dprWatcherDispose = undefined;
    clearFeatureCountsForPrefix('survey-');
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
    this.styleRestoreDispose?.();
    this.styleRestoreDispose = undefined;

    // Drop the module-level VT registry entries + zero the memory-probe counts.
    // (releaseVtRegistry also disposes the DPR watcher, covering both this path
    // and MapCanvas's lightweight unmount.)
    this.releaseVtRegistry();

    // Cancel any pending hover RAF
    if (this.hoverRafId !== null) {
      cancelAnimationFrame(this.hoverRafId);
      this.hoverRafId = null;
    }
    this.pendingHoverEvent = null;
    this.cancelDragRender();
    // Tear down the ghost (custom layer / stock follow listener + timer) so
    // nothing fires on a torn-down map.
    this.dragGhost.end();

    // Drop the touch long-press listeners and any pending hold. Done before the
    // live-style guard below, not inside it: the helper holds the canvas element
    // itself, so it detaches cleanly even once the map is gone.
    this.detachLongPressTouch?.();
    this.detachLongPressTouch = null;

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
        this.map.getCanvas().removeEventListener('mouseleave', this.boundMapLeave);
      }
      this.map.off('zoomend', this.handleZoomEndTelemetry);
      this.map.off('moveend', this.handleMoveEndTelemetry);

      // Remove all layers first (must be done before removing their sources).
      // The custom survey geometry / labels / measured-path layers are removed by
      // their controllers' destroy() calls above; only the auxiliary GeoJSON
      // overlays are owned here.
      if (this.map.getLayer(this.stationsHighlightLayerId)) {
        this.map.removeLayer(this.stationsHighlightLayerId);
      }
      if (this.map.getLayer(this.gasRouteLayerId)) {
        this.map.removeLayer(this.gasRouteLayerId);
      }
      // Interact overlay layers (#mem)
      if (this.map.getLayer(this.interactShotsLayerId)) {
        this.map.removeLayer(this.interactShotsLayerId);
      }
      if (this.map.getLayer(this.interactShotsMeasuredLayerId)) {
        this.map.removeLayer(this.interactShotsMeasuredLayerId);
      }
      if (this.map.getLayer(this.interactStationsLayerId)) {
        this.map.removeLayer(this.interactStationsLayerId);
      }
      if (this.map.getLayer(this.loopClosuresLayerId)) {
        this.map.removeLayer(this.loopClosuresLayerId);
      }
      // Remove ghost section layers
      if (this.map.getLayer(this.ghostShotsLayerId)) {
        this.map.removeLayer(this.ghostShotsLayerId);
      }
      if (this.map.getLayer(this.ghostStationsLayerId)) {
        this.map.removeLayer(this.ghostStationsLayerId);
      }
      if (this.map.getLayer(this.ghostMergeLayerId)) {
        this.map.removeLayer(this.ghostMergeLayerId);
      }
      // Remove ghost hitbox layers (the main hitbox layers were removed, #mem)
      if (this.map.getLayer(this.ghostStationsHitboxLayerId)) {
        this.map.removeLayer(this.ghostStationsHitboxLayerId);
      }
      if (this.map.getLayer(this.ghostShotsHitboxLayerId)) {
        this.map.removeLayer(this.ghostShotsHitboxLayerId);
      }

      // Remove all sources after layers are removed
      if (this.map.getSource(this.sourceId + '-loop-closures')) {
        this.map.removeSource(this.sourceId + '-loop-closures');
      }
      if (this.map.getSource(this.sourceId + '-gas-route')) {
        this.map.removeSource(this.sourceId + '-gas-route');
      }
      // Remove ghost section sources
      if (this.map.getSource(this.sourceId + '-ghost-shots')) {
        this.map.removeSource(this.sourceId + '-ghost-shots');
      }
      if (this.map.getSource(this.sourceId + '-ghost-stations')) {
        this.map.removeSource(this.sourceId + '-ghost-stations');
      }
      if (this.map.getSource(this.sourceId + '-ghost-merge')) {
        this.map.removeSource(this.sourceId + '-ghost-merge');
      }
      if (this.map.getSource(this.interactSourceId)) {
        this.map.removeSource(this.interactSourceId);
      }
    } catch {
      // Map already destroyed during unmount, cleanup not needed
    }

    // Clear bound handler references
    this.boundMouseMove = null;
    this.boundMapLeave = null;
  }
}
