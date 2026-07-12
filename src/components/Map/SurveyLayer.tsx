import type { Map as MaplibreMap, MapMouseEvent, ExpressionSpecification } from 'maplibre-gl';
import type maplibregl from 'maplibre-gl';
import type {
  LoopClosure,
  Marker,
  Section,
  SectionId,
  ShotId,
  Station,
  StationRef,
} from '../../types/project';
import type { Config, GpsOrigin, DecimalPrecision } from '../../types/config';
import type { RouteLeg } from '../../types/ipc.generated';
import { isNonInteractiveTool } from '../../utils/toolUtils';
import type { ToolType } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { selectActiveSurveyData } from '../../store/geometrySelectors';
import { surveyDataFromLegacy } from '../../types/survey-data';
import { hasLiveStyle } from './mapUtils';
import {
  buildGhostFeatures,
  buildSurveyFeatures,
  normalizeDepthRange,
  type DepthRange,
  type GhostSectionData,
} from './surveyLayer/features';
import { computeIssueSeverity } from './surveyLayer/severity';
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
import { formatFixed } from '../../i18n/format';
import { getErrorColor } from '../../utils/colors';
import { trackNav } from '../../utils/telemetry';
import { FEATURE_ID_STRIDE } from '../../utils/ids';
import { MEASURE_TINT } from '../../utils/measurePath';
import {
  distanceToLineSegment,
  projectParamOnSegment,
  resolveNearestShot,
  resolveNearestStation,
} from './surveyLayer/hitTest';
import { recordFeatureCount, clearFeatureCountsForPrefix, recordPerfTiming } from '../../utils/memoryProbe';
import { buildSurveyFeaturesAsync } from './surveyLayer/featuresWorkerClient';
// Note: buildPriorityExpression, compositeIdEquals, inSection no longer needed
// after switching to setFeatureState for hover/selection highlighting

/** Comment-label halo width (#52): the plain base width and the wider,
 *  more-readable width while `move_comment` is tinting the labels. Shared by
 *  the initial layer defs and the tool-transition restore path so they can't
 *  drift apart. */
const COMMENT_HALO_WIDTH_BASE = 1.5;
const COMMENT_HALO_WIDTH_TINTED = 2.5;

export interface SurveyLayerCallbacks {
  onStationClick?: (ref: StationRef, event: MapMouseEvent) => void;
  onShotClick?: (sectionId: SectionId, shotId: ShotId, event: MapMouseEvent) => void;
  onMarkerClick?: (sectionId: SectionId, shotId: ShotId) => void;
  onEmptyClick?: (event: MapMouseEvent) => void;
  onStationHover?: (ref: StationRef | null) => void;
  onShotHover?: (shot: { sectionId: SectionId; shotId: ShotId } | null) => void;
  onFeatureHover?: ((isHovering: boolean) => void) | undefined;
  onContextMenu?: (
    target: { type: 'station'; ref: StationRef } | { type: 'shot'; sectionId: SectionId; shotId: ShotId } | null,
    event: MapMouseEvent
  ) => void;
  /** Called when user clicks near a hidden station (for toast feedback) */
  onHiddenStationClick?: () => void;
  /** Called when user clicks near a hidden shot (for toast feedback) */
  onHiddenShotClick?: () => void;
}

/**
 * Survey layer that renders shots and stations on the map
 *
 * This is a simplified implementation using MapLibre GL JS.
 * For better performance with large datasets, this could be
 * replaced with deck.gl layers.
 */
export type { GhostSectionData } from './surveyLayer/features';

export default class SurveyLayer {
  private map: MaplibreMap;
  private sourceId = 'survey-data';
  private shotsLayerId = 'survey-shots';
  private shotGradientsLayerId = 'survey-shot-gradients';
  // Measure along-line tint overlay (#26): the measured path's shots redrawn ABOVE
  // the depth-gradient sub-segments so the tint is never occluded by them.
  private measurePathLayerId = 'survey-measure-path';
  private stationsLayerId = 'survey-stations';
  private labelsLayerId = 'survey-labels';
  private shotLabelsLayerId = 'survey-shot-labels';
  private stationLabelsLayerId = 'survey-station-labels';
  private stationCommentsLayerId = 'survey-station-comments';
  private shotCommentsLayerId = 'survey-shot-comments';
  private loopClosuresLayerId = 'survey-loop-closures';
  private dragClosuresLayerId = 'survey-drag-loop-closures';
  private markersLayerId = 'survey-markers';
  private markerLabelsLayerId = 'survey-marker-labels';
  private ghostShotsLayerId = 'survey-ghost-shots';
  private ghostStationsLayerId = 'survey-ghost-stations';
  private ghostMergeLayerId = 'survey-ghost-merge';
  private stationsHighlightLayerId = 'survey-stations-highlight';
  // Hitbox layers for improved click detection
  private stationsHitboxLayerId = 'survey-stations-hitbox';
  private shotsHitboxLayerId = 'survey-shots-hitbox';
  // Gas route highlight (#233): the picked penetration route drawn over the survey — one line
  // layer, swum legs at full opacity and jump legs faded via a data-driven line-opacity (#243).
  private gasRouteLayerId = 'survey-gas-route';

  // Base zoom-density filters for label layers (set at init, restored when clearing reference filter)
  private shotLabelsBaseFilter: maplibregl.FilterSpecification | null = null;
  private stationLabelsBaseFilter: maplibregl.FilterSpecification | null = null;
  private ghostStationsHitboxLayerId = 'survey-ghost-stations-hitbox';
  private ghostShotsHitboxLayerId = 'survey-ghost-shots-hitbox';
  private callbacks: SurveyLayerCallbacks;

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

  // Hidden comment during move_comment pickup (visually hides without backend mutation)
  private hiddenComment: { type: 'station' | 'shot'; sectionId: number; id: number } | null = null;
  // Cached reference-shots visibility for recomputing comment filters
  private showReferenceShotsCached = true;

  // Comment tool override: when comment/move_comment tool is active, force both
  // comment layers visible with text-allow-overlap so comments win over other labels.
  private commentToolActive = false;
  private cachedShowStationComments = false;
  private cachedShowShotComments = false;

  // Active tool state (for disabling interactions when non-interactive tools are active)
  private activeTool: ToolType = 'select';

  // Anchored section IDs (for fading when Move Segment tool is active)
  private anchoredSectionIds: Set<number> = new Set();
  private prevAnchoredFeatureIds: Set<number> = new Set();

  // Immovable feature IDs for Move Station / Move Shot tools
  // (per-feature granularity, unlike anchoredSectionIds which is per-section)
  private immovableStationFeatureIds: Set<number> = new Set();
  private immovableShotFeatureIds: Set<number> = new Set();

  // Drag preview state — uses temporary layers with *-translate paint properties
  // instead of rebuilding GeoJSON on every frame
  private dragPreviewActive = false;
  private dragSavedFilters = new Map<string, maplibregl.FilterSpecification | undefined>();
  // One-shot callback fired at end of update() — used to restore filters after
  // source data has been updated, in the same synchronous block (no paint gap).
  private onNextUpdateCallback: (() => void) | null = null;

  /**
   * setData coalescer: if multiple update() calls land in the same microtask
   * (typical of React effect cascades — depthRange/gpsOrigin/precision refs
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
    markers: Marker[] | undefined;
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
  /** Monotonic id incremented on every update() flush start. Async worker
   *  builds compare against this on resolve to bail if a newer update
   *  invalidated them mid-flight. */
  private lastUpdateToken = 0;
  // Layers that participate in drag preview (main layer → temp layer config)
  private static readonly DRAG_LAYER_DEFS: Array<{
    mainLayerId: string;
    tempLayerId: string;
    sourceId: string;
    type: 'line' | 'circle' | 'symbol';
    translateProp: string;
    translateAnchorProp: string;
  }> = [
    { mainLayerId: 'survey-shots', tempLayerId: 'survey-drag-shots', sourceId: 'survey-data-shots', type: 'line', translateProp: 'line-translate', translateAnchorProp: 'line-translate-anchor' },
    { mainLayerId: 'survey-shot-gradients', tempLayerId: 'survey-drag-shot-gradients', sourceId: 'survey-data-shot-gradients', type: 'line', translateProp: 'line-translate', translateAnchorProp: 'line-translate-anchor' },
    { mainLayerId: 'survey-stations', tempLayerId: 'survey-drag-stations', sourceId: 'survey-data-stations', type: 'circle', translateProp: 'circle-translate', translateAnchorProp: 'circle-translate-anchor' },
    { mainLayerId: 'survey-shot-labels', tempLayerId: 'survey-drag-shot-labels', sourceId: 'survey-data-shot-labels', type: 'symbol', translateProp: 'text-translate', translateAnchorProp: 'text-translate-anchor' },
    { mainLayerId: 'survey-labels', tempLayerId: 'survey-drag-labels', sourceId: 'survey-data-section-labels', type: 'symbol', translateProp: 'text-translate', translateAnchorProp: 'text-translate-anchor' },
    { mainLayerId: 'survey-station-labels', tempLayerId: 'survey-drag-station-labels', sourceId: 'survey-data-station-labels', type: 'symbol', translateProp: 'text-translate', translateAnchorProp: 'text-translate-anchor' },
  ];

  // Station drag preview — dynamic geometry rebuild for connected shots
  // (translate is insufficient because only one endpoint of each shot moves)
  private stationDragShots: Array<{
    properties: Record<string, unknown>;
    fromCoord: [number, number]; // original [lng, lat]
    toCoord: [number, number];
    movingEnd: 'from' | 'to' | 'both';
    shotLabelProperties: Record<string, unknown>;
  }> | null = null;
  private static readonly STATION_DRAG_SHOT_SOURCE = 'survey-drag-station-shots-src';
  private static readonly STATION_DRAG_SHOT_LAYER = 'survey-drag-station-shots-layer';
  private static readonly STATION_DRAG_SHOT_LABEL_SOURCE = 'survey-drag-station-shot-labels-src';
  private static readonly STATION_DRAG_SHOT_LABEL_LAYER = 'survey-drag-station-shot-labels-layer';

  // Station drag preview — dynamic geometry rebuild for connected closure indicator lines
  // (translate is insufficient because only one endpoint of each closure line may move)
  private stationDragClosures: Array<{
    properties: Record<string, unknown>;
    fromCoord: [number, number]; // original [lng, lat] — station_a
    toCoord: [number, number];   // original [lng, lat] — station_b
    movingEnd: 'from' | 'to' | 'both';
  }> | null = null;
  private static readonly STATION_DRAG_CLOSURE_SOURCE = 'survey-drag-station-closures-src';
  private static readonly STATION_DRAG_CLOSURE_LAYER = 'survey-drag-station-closures-layer';

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
  // feature rebuild; re-applied in updateVisualState() because MapLibre clears
  // feature-state on every setData().
  private distanceMode: boolean = false;
  private distanceState: DistanceFeatureState | null = null;
  // Gas reachable-frontier overlay (#38). Same feature-state mechanics as the
  // distance gradient (own `gasReach` key); mutually exclusive with it (both own
  // the shots/stations color paint), enforced upstream in uiStore.
  private gasReachMode: boolean = false;
  private gasReachState: GasReachFeatureState | null = null;
  // Gas route highlight (#233): the current picked-route legs. Re-applied after every setData()
  // rebuild (its own GeoJSON source survives setData, but a feature rebuild re-pushes the data).
  private gasRouteLegs: RouteLeg[] = [];
  // Measured along-line path (#26): the path's shots AND stations are tinted via
  // a `measured` feature-state + a paint `case` on shots line-color / stations
  // circle-color, in a user-configurable color. Re-applied after setData() (which
  // clears feature-state), and suppressed while the distance gradient owns color.
  private measuredPathIds: Set<number> | null = null;
  private measuredPathStationIds: Set<number> | null = null;
  private measureTintColor: string = MEASURE_TINT;

  // Theme colors for MapLibre paint properties (MapLibre cannot read CSS vars)
  // Seeded at construction so first paint uses current theme, not legacy fallbacks
  private themeColors: MapThemeColors = getMapThemeColors();

  // Cached input data for feature rebuild on theme change
  private cachedSections: Section[] = [];
  private cachedStations: Station[] = [];
  private cachedLoopClosures: LoopClosure[] | undefined;
  private cachedMarkers: Marker[] | undefined;
  private cachedPrecision: DecimalPrecision | undefined;

  // Lazy label sources. shot-labels and station-labels both have minzoom 14
  // (so do the comment-overlay layers that share those sources). Building
  // ~75k+89k label features at lower zooms wastes ~100 MB main+worker on
  // the load path. We track the last built state with hysteresis to avoid
  // thrashing if the user zooms across the threshold rapidly.
  private static readonly LABEL_BUILD_ZOOM = 13.5;
  private static readonly LABEL_DROP_ZOOM = 13;
  /** Whether label features are currently materialized in the sources. */
  private labelsCurrentlyBuilt = true;
  /** Bound handler for map.on('zoomend') — kept for proper off() in destroy. */
  private boundZoomEndForLabels: (() => void) | null = null;

  // Bound event handlers stored for proper cleanup in destroy()
  private boundMouseMove: ((e: maplibregl.MapMouseEvent) => void) | null = null;
  private boundMapLeave: (() => void) | null = null;

  // Touch long-press state for context menu
  private touchLongPressTimer: ReturnType<typeof setTimeout> | null = null;
  private touchStartPos: { x: number; y: number } | null = null;
  private boundTouchStart: ((e: TouchEvent) => void) | null = null;
  private boundTouchMove: ((e: TouchEvent) => void) | null = null;
  private boundTouchEnd: ((e: TouchEvent) => void) | null = null;

  // Hover update batching - only process latest mouse position per frame
  private pendingHoverEvent: maplibregl.MapMouseEvent | null = null;
  private hoverRafId: number | null = null;

  // Previous feature IDs for setFeatureState (to clear state when hover/selection changes)
  private prevHoveredStationFeatureId: number | null = null;
  private prevHoveredShotFeatureId: number | null = null;
  private prevSelectedStationFeatureId: number | null = null;
  private prevSelectedShotFeatureId: number | null = null;
  private prevSnappedStationFeatureId: number | null = null;

  // Previous section-level feature IDs for setFeatureState (to clear state when section hover/selection changes)
  private prevHoveredSectionShotIds: Set<number> = new Set();
  private prevHoveredSectionStationIds: Set<number> = new Set();
  private prevSelectedSectionShotIds: Set<number> = new Set();
  private prevSelectedSectionStationIds: Set<number> = new Set();

  // Pre-built section→featureId lookup maps, populated once in update() from feature data.
  // Eliminates O(N) querySourceFeatures calls during hover (the main perf bottleneck).
  private sectionShotFeatureIds: Map<number, Set<number>> = new Map();
  private sectionStationFeatureIds: Map<number, Set<number>> = new Map();

  // Cached section feature IDs to avoid re-scanning on every updateVisualState()
  // Key is comma-joined section IDs (handles both single-section and multi-section/segment hover)
  private cachedHoveredSectionKey: string = '';
  private cachedHoveredSectionShotIds: Set<number> = new Set();
  private cachedHoveredSectionStationIds: Set<number> = new Set();
  private cachedSelectedSectionId: string | null = null;
  private cachedSelectedSectionShotIds: Set<number> = new Set();
  private cachedSelectedSectionStationIds: Set<number> = new Set();

  constructor(map: MaplibreMap, callbacks: SurveyLayerCallbacks = {}) {
    this.map = map;
    this.callbacks = callbacks;
    this.initializeSources();
    this.initializeLayers();
    this.initializeEventHandlers();
  }

  setCallbacks(callbacks: SurveyLayerCallbacks) {
    this.callbacks = callbacks;
  }

  // Config setter - triggers layer updates when config changes
  public setConfig(config: Config): void {
    this.config = config;
    this.updateHitboxLayers();
    this.updateGhostVisualLayers();
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
      this.updateVisualState();
      this.map.getCanvas().style.cursor = '';

      // CRITICAL: Also notify callbacks to clear MapCanvas state
      this.callbacks.onFeatureHover?.(false);
      this.callbacks.onStationHover?.(null);
      this.callbacks.onShotHover?.(null);
    }

    // Move Station/Shot tools: set arrow cursor immediately on activation
    // (prevents MapLibre's grab cursor from showing before any hover event fires)
    if (tool === 'move_station' || tool === 'move_shot') {
      this.map.getCanvas().style.cursor = 'default';
    } else if (!isNonInteractive) {
      // Reset cursor when switching away from move tools to other interactive tools
      this.map.getCanvas().style.cursor = '';
    }

    // Comment tool override: force comment layers visible + overlap priority
    const isCommentTool = tool === 'comment' || tool === 'move_comment';
    if (isCommentTool && !this.commentToolActive) {
      this._enableCommentToolOverride();
    } else if (!isCommentTool && this.commentToolActive) {
      this._disableCommentToolOverride();
    }

    // Move-comment palette tint (#52): exactly `move_comment` (not the plain
    // `comment` tool) tints each comment label and its owning shot/station.
    // Re-derive the paint expressions only on transition in/out - they're
    // computed on demand from current state, so this also naturally restores
    // whatever the shots/stations color owned before (section/depth/measured
    // tint, or an analytical overlay, which the tint yields to).
    const isMoveComment = tool === 'move_comment';
    if (isMoveComment !== wasMoveComment) {
      this.applyOverlayPaint();
      this.applyCommentToolTint();
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
    // Large buffer prevents tile-boundary clipping when features are translated
    // during segment drag preview. Default is 128; 512 = full tile width.
    // Cave survey data is small so the extra tiling cost is negligible.
    const dragBuffer = 512;

    // Shots line source - promoteId enables setFeatureState for hover/selection
    this.map.addSource(this.sourceId + '-shots', {
      type: 'geojson',
      promoteId: 'featureId',
      buffer: dragBuffer,
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    // Stations point source - promoteId enables setFeatureState for hover/selection
    this.map.addSource(this.sourceId + '-stations', {
      type: 'geojson',
      promoteId: 'featureId',
      buffer: dragBuffer,
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    // Shot gradient segment source (visual-only overlay for per-shot depth transitions)
    this.map.addSource(this.sourceId + '-shot-gradients', {
      type: 'geojson',
      buffer: dragBuffer,
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    // Loop closures line source
    this.map.addSource(this.sourceId + '-loop-closures', {
      type: 'geojson',
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    // Markers point source
    this.map.addSource(this.sourceId + '-markers', {
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

    // Shot labels point source (midpoint of each shot)
    this.map.addSource(this.sourceId + '-shot-labels', {
      type: 'geojson',
      buffer: dragBuffer,
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    // Section name labels point source (first station of each section)
    this.map.addSource(this.sourceId + '-section-labels', {
      type: 'geojson',
      buffer: dragBuffer,
      data: {
        type: 'FeatureCollection',
        features: [],
      },
    });

    // Station index labels point source (at each station)
    this.map.addSource(this.sourceId + '-station-labels', {
      type: 'geojson',
      buffer: dragBuffer,
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
  }

  private initializeLayers() {
    // === HITBOX LAYERS (invisible, for click detection) ===
    // Layer order (bottom to top):
    // 1. survey-shots-hitbox
    // 2. survey-stations-hitbox (above shots for hover priority)
    // 3. survey-ghost-shots-hitbox (debug only)
    // 4. survey-ghost-stations-hitbox (debug only)
    // Then visual layers follow...

    // Shot hitbox layer - added first (bottom of stack)
    // Use opaque color with tiny opacity for reliable queryRenderedFeatures
    this.map.addLayer({
      id: this.shotsHitboxLayerId,
      type: 'line',
      source: this.sourceId + '-shots',
      layout: {
        'line-cap': 'round', // Better endpoint detection
      },
      paint: {
        'line-width': this.getShotHitArea(),
        'line-color': '#000000', // Opaque color - use opacity for invisibility
        'line-opacity': 0.001, // CRITICAL: non-zero for queryRenderedFeatures
      },
    });

    // Station hitbox layer - above shots for correct hover priority
    // Use opaque color with tiny opacity for reliable queryRenderedFeatures
    this.map.addLayer({
      id: this.stationsHitboxLayerId,
      type: 'circle',
      source: this.sourceId + '-stations',
      paint: {
        'circle-radius': this.getStationHitArea(),
        'circle-color': '#000000', // Opaque color - use opacity for invisibility
        'circle-opacity': 0.001, // CRITICAL: non-zero for queryRenderedFeatures
        'circle-stroke-width': 0,
      },
    });

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

    // Markers circle layer -- positioned at the interpolated point along a shot
    this.map.addLayer({
      id: this.markersLayerId,
      type: 'circle',
      source: this.sourceId + '-markers',
      paint: {
        'circle-radius': 6,
        'circle-color': ['get', 'color'],
        'circle-stroke-width': 2,
        'circle-stroke-color': this.themeColors.strokeColor,
        'circle-opacity': 1,
      },
    });

    // Marker text labels -- only shown when the marker has non-empty text
    this.map.addLayer({
      id: this.markerLabelsLayerId,
      type: 'symbol',
      source: this.sourceId + '-markers',
      layout: {
        'text-field': ['get', 'text'],
        'text-font': ['Open Sans Regular'],
        'text-size': 11,
        'text-offset': [0, -1.4],
        'text-anchor': 'bottom',
        'text-allow-overlap': false,
      },
      paint: {
        'text-color': ['get', 'color'],
        'text-halo-color': this.themeColors.textHaloAlt,
        'text-halo-width': 1.5,
      },
      filter: ['!=', ['get', 'text'], ''],
    });

    // Shots layer (lines) - uses feature-state for hover/selection
    const baseWidth = this.getShotWidth();
    const hoverWidth = this.getShotHoverWidth();
    const selectionWidth = this.getShotSelectionWidth();

    this.map.addLayer({
      id: this.shotsLayerId,
      type: 'line',
      source: this.sourceId + '-shots',
      paint: {
        'line-color': ['get', 'color'],
        // Width: hover > selected > base (uses feature-state)
        'line-width': [
          'case',
          ['boolean', ['feature-state', 'hover'], false], hoverWidth,
          ['boolean', ['feature-state', 'selected'], false], selectionWidth,
          baseWidth,
        ],
        'line-opacity': [
          'case',
          ['boolean', ['feature-state', 'anchored'], false], 0.25,
          ['get', 'isReference'], 0.5,
          1,
        ],
      },
    });

    // Gradient overlay (composed of tiny line segments) for true in->out shot depth transitions.
    // Uses a slightly smaller width so hover/selection states on the main shots layer remain visible.
    this.map.addLayer({
      id: this.shotGradientsLayerId,
      type: 'line',
      source: this.sourceId + '-shot-gradients',
      layout: {
        'line-cap': 'round',
        'line-join': 'round',
      },
      paint: {
        'line-color': ['get', 'color'],
        'line-width': Math.max(1, baseWidth - 0.35),
        'line-opacity': [
          'case',
          ['get', 'isReference'], 0.5,
          1,
        ],
      },
    });

    // Measure along-line tint overlay (#26): the measured path's shots, drawn on
    // top of the depth-gradient sub-segments so the highlight is never occluded by
    // them. Visibility is driven by the same `measured` feature-state as the main
    // shots layer (opacity 1 when measured, else 0), so non-path shots stay
    // invisible here and keep their normal/gradient color underneath. Width matches
    // the main shots line so it fully covers the (narrower) gradient.
    this.map.addLayer({
      id: this.measurePathLayerId,
      type: 'line',
      source: this.sourceId + '-shots',
      layout: {
        'line-cap': 'round',
        'line-join': 'round',
      },
      paint: {
        'line-color': this.measureTintColor,
        'line-width': [
          'case',
          ['boolean', ['feature-state', 'hover'], false], hoverWidth,
          ['boolean', ['feature-state', 'selected'], false], selectionWidth,
          baseWidth,
        ],
        'line-opacity': [
          'case',
          ['boolean', ['feature-state', 'measured'], false], 1,
          0,
        ],
      },
    });

    // Stations layer (circles)
    // Uses feature-state for efficient hover/selection highlighting
    const baseRadius = this.getStationRadius();
    const hoverRadius = this.getStationHoverRadius();
    const selectionRadius = this.getStationSelectionRadius();

    this.map.addLayer({
      id: this.stationsLayerId,
      type: 'circle',
      source: this.sourceId + '-stations',
      paint: {
        // Radius: hover > selected > zeroDatum > base (uses feature-state)
        'circle-radius': [
          'case',
          ['boolean', ['feature-state', 'hover'], false], hoverRadius,
          ['boolean', ['feature-state', 'selected'], false], selectionRadius,
          ['get', 'isZeroDatum'], baseRadius * 2,
          baseRadius,
        ],
        // Color from feature property
        'circle-color': ['get', 'color'],
        // Opacity: reduced for anchored sections
        'circle-opacity': [
          'case',
          ['boolean', ['feature-state', 'anchored'], false], 0.25,
          1,
        ],
        // Stroke width: hover/selected states get thicker strokes (uses feature-state)
        'circle-stroke-width': [
          'case',
          ['boolean', ['feature-state', 'hover'], false], 4,
          ['boolean', ['feature-state', 'selected'], false], 2.5,
          ['get', 'isZeroDatum'], 2,
          1,
        ],
        // Stroke color: hover=highlight, selected=select, else from property
        'circle-stroke-color': [
          'case',
          ['boolean', ['feature-state', 'hover'], false], this.themeColors.hoverHighlight,
          ['boolean', ['feature-state', 'selected'], false], this.themeColors.selectHighlight,
          ['get', 'strokeColor'],
        ],
        // Stroke opacity: reduced for anchored sections
        'circle-stroke-opacity': [
          'case',
          ['boolean', ['feature-state', 'anchored'], false], 0.25,
          1,
        ],
      },
    });

    // Station highlight layer for snapped/active stations
    // Inserted immediately after stations - labels added later render on top naturally
    // Radius is config-based (baseRadius + 3), updated via updateVisualState()
    this.map.addLayer({
      id: this.stationsHighlightLayerId,
      type: 'circle',
      source: this.sourceId + '-stations',
      paint: {
        'circle-radius': this.getStationRadius() + 3, // Snapped state: radius + 3
        'circle-color': this.themeColors.snapColor,
        'circle-stroke-width': 4,
        'circle-stroke-color': this.themeColors.snapColor,
        // Visibility driven by feature-state — same instant mechanism as hover.
        // setFeatureState is a per-feature GPU update, unlike setFilter which
        // forces filter re-evaluation across all features.
        'circle-opacity': ['case', ['boolean', ['feature-state', 'snapped'], false], 1, 0],
        'circle-stroke-opacity': ['case', ['boolean', ['feature-state', 'snapped'], false], 1, 0],
        // Disable default 300ms paint transitions — snap indicator must appear/disappear instantly
        'circle-opacity-transition': { duration: 0, delay: 0 },
        'circle-stroke-opacity-transition': { duration: 0, delay: 0 },
      },
    });

    // Shot data labels layer - shows azimuth, length, depth at shot midpoint
    // Visibility controlled by showShotLabels toggle and zoom-based density
    this.map.addLayer({
      id: this.shotLabelsLayerId,
      type: 'symbol',
      source: this.sourceId + '-shot-labels',
      minzoom: 14, // Performance: don't render at low zooms
      layout: {
        'text-field': ['get', 'label'],
        'text-font': ['Open Sans Regular'],
        'text-size': [
          'interpolate',
          ['linear'],
          ['zoom'],
          14, 9,
          18, 12,
        ],
        'text-offset': [0, 0.5],
        'text-anchor': 'top',
        // Progressive density: hide labels at low zoom
        'text-allow-overlap': false,
        'visibility': 'none', // Start hidden, controlled by setShowShotLabels
      },
      paint: {
        'text-color': ['get', 'color'], // Dynamic color for depth gradient support
        'text-halo-color': this.themeColors.textHaloAlt,
        'text-halo-width': 2, // Increased for readability with depth colors
        'text-opacity': [
          'interpolate',
          ['linear'],
          ['zoom'],
          14, 0.7,
          18, 1,
        ],
      },
      filter: this.shotLabelsBaseFilter = [
        'case',
        // At zoom < 14: show nothing (filter all)
        ['<', ['zoom'], 14],
        false,
        // At zoom 14-16: show every 5th label (index % 5 == 0)
        ['all', ['>=', ['zoom'], 14], ['<', ['zoom'], 16]],
        ['==', ['%', ['get', 'index'], 5], 0],
        // At zoom 16-18: show every 2nd label
        ['all', ['>=', ['zoom'], 16], ['<', ['zoom'], 18]],
        ['==', ['%', ['get', 'index'], 2], 0],
        // At zoom >= 18: show all
        true,
      ],
    });

    // Gas route highlight (#233): the picked route drawn over the survey. Jump legs are
    // faded rather than dashed (#243) — the same half-opacity treatment reference shots
    // get — and unlike `line-dasharray`, `line-opacity` IS data-driven, so one layer
    // covers both leg kinds.
    this.map.addLayer({
      id: this.gasRouteLayerId,
      type: 'line',
      source: this.sourceId + '-gas-route',
      paint: {
        'line-color': '#22d3ee',
        'line-width': 4,
        'line-opacity': ['case', ['==', ['get', 'isJump'], true], 0.45, 0.9],
      },
    });

    // Section name labels layer - shows section name at first station
    // Visibility controlled by showSectionNames toggle
    this.map.addLayer({
      id: this.labelsLayerId,
      type: 'symbol',
      source: this.sourceId + '-section-labels',
      minzoom: 10, // Performance: don't render at very low zooms
      layout: {
        'text-field': ['get', 'name'],
        'text-font': ['Open Sans Regular'],
        'text-size': [
          'interpolate',
          ['linear'],
          ['zoom'],
          12, 11,
          18, 14,
        ],
        'text-offset': [0, -1.2],
        'text-anchor': 'bottom',
        'text-allow-overlap': false, // Avoid clutter in dense areas
        'visibility': 'visible', // Start visible, controlled by setShowSectionNames
      },
      paint: {
        'text-color': ['get', 'color'],
        'text-halo-color': this.themeColors.textHalo,
        'text-halo-width': 1.5,
        'text-opacity': 1,
      },
    });

    // Station index labels layer - shows station index (0, 1, 2...) near each station
    // Visibility controlled by showStationLabels toggle and zoom-dependent density
    this.map.addLayer({
      id: this.stationLabelsLayerId,
      type: 'symbol',
      source: this.sourceId + '-station-labels',
      minzoom: 14, // Performance: don't render at low zooms
      layout: {
        'text-field': ['get', 'label'],
        'text-font': ['Open Sans Regular'],
        'text-size': [
          'interpolate',
          ['linear'],
          ['zoom'],
          14, 9,
          18, 11,
        ],
        'text-offset': [0.8, -0.8], // Offset to top-right of station dot
        'text-anchor': 'bottom-left',
        'text-allow-overlap': false, // Avoid clutter with overlapping labels
        'visibility': 'none', // Start hidden, controlled by setShowStationLabels
      },
      paint: {
        'text-color': ['get', 'color'], // Dynamic color for depth gradient support
        'text-halo-color': this.themeColors.textHaloAlt,
        'text-halo-width': 2, // Increased for readability with depth colors
        'text-opacity': [
          'interpolate',
          ['linear'],
          ['zoom'],
          14, 0.7,
          18, 1,
        ],
      },
      // Zoom-dependent filtering using globalOrdinal (stable sequential index)
      // Always show first station in each section (isFirstInSection) to avoid empty-looking sections
      filter: this.stationLabelsBaseFilter = [
        'case',
        // At zoom < 14: show nothing
        ['<', ['zoom'], 14],
        false,
        // At zoom 14-16: show first station + every 5th (globalOrdinal % 5 == 0)
        ['all', ['>=', ['zoom'], 14], ['<', ['zoom'], 16]],
        ['any', ['get', 'isFirstInSection'], ['==', ['%', ['get', 'globalOrdinal'], 5], 0]],
        // At zoom 16-18: show first station + every 2nd
        ['all', ['>=', ['zoom'], 16], ['<', ['zoom'], 18]],
        ['any', ['get', 'isFirstInSection'], ['==', ['%', ['get', 'globalOrdinal'], 2], 0]],
        // At zoom >= 18: show all
        true,
      ],
    });

    // Station comments layer - shows comment text near each station
    // Uses the same source as station labels, filtered to only show stations with comments
    this.map.addLayer({
      id: this.stationCommentsLayerId,
      type: 'symbol',
      source: this.sourceId + '-station-labels',
      minzoom: 14,
      layout: {
        'text-field': ['get', 'comment'],
        'text-font': ['Open Sans Regular'],
        'text-size': [
          'interpolate',
          ['linear'],
          ['zoom'],
          14, 8,
          18, 10,
        ],
        'text-offset': [0, 1.5], // Below station dot
        'text-anchor': 'top',
        'text-allow-overlap': false,
        'text-max-width': 12,
        'visibility': 'none', // Start hidden, controlled by setShowStationComments
      },
      paint: {
        'text-color': this.commentTextColor(),
        'text-halo-color': this.themeColors.textHaloAlt,
        'text-halo-width': COMMENT_HALO_WIDTH_BASE,
        'text-opacity': 0.85,
      },
      filter: ['!=', ['get', 'comment'], null],
    });

    // Shot comments layer - shows comment text near each shot midpoint
    // Uses the same source as shot labels, filtered to only show shots with comments
    this.map.addLayer({
      id: this.shotCommentsLayerId,
      type: 'symbol',
      source: this.sourceId + '-shot-labels',
      minzoom: 14,
      layout: {
        'text-field': ['get', 'comment'],
        'text-font': ['Open Sans Regular'],
        'text-size': [
          'interpolate',
          ['linear'],
          ['zoom'],
          14, 8,
          18, 10,
        ],
        'text-offset': [0, 1.5], // Below shot midpoint
        'text-anchor': 'top',
        'text-allow-overlap': false,
        'text-max-width': 12,
        'visibility': 'none', // Start hidden, controlled by setShowShotComments
      },
      paint: {
        'text-color': this.commentTextColor(),
        'text-halo-color': this.themeColors.textHaloAlt,
        'text-halo-width': COMMENT_HALO_WIDTH_BASE,
        'text-opacity': 0.85,
      },
      filter: ['!=', ['get', 'comment'], null],
    });

  }

  private initializeEventHandlers() {
    // UNIFIED click handler - queries both hitbox layers with station priority
    // This replaces per-layer click handlers to ensure proper nearest-to-click resolution
    this.map.on('click', this.handleMapClick);

    // Note: Per-layer click handlers removed - unified handler handles all click logic

    // Process hover update for a given mouse event
    const processHoverUpdate = (e: maplibregl.MapMouseEvent) => {
      // Query both hitbox layers in a single call for better performance
      const allFeatures = this.map.queryRenderedFeatures(e.point, {
        layers: [this.stationsHitboxLayerId, this.shotsHitboxLayerId],
      });

      // Filter results by layer type
      const stationFeatures = allFeatures.filter(
        (f) => f.layer.id === this.stationsHitboxLayerId
      );
      const shotFeatures = allFeatures.filter(
        (f) => f.layer.id === this.shotsHitboxLayerId
      );

      // Resolve nearest of each type (respect config hit areas)
      const stationHitArea = this.getStationHitArea();
      const shotHitArea = this.getShotHitArea();
      const nearestStation = resolveNearestStation(this.map, e, stationFeatures, stationHitArea);
      const nearestShot = resolveNearestShot(this.map, e, shotFeatures, shotHitArea);

      // Determine what to highlight (stations take priority unless suppressed by tool)
      let newHoveredStationId: string | null = null;
      let newHoveredShotId: string | null = null;

      // Tool-specific hover suppression:
      // - move_station: only stations hover (suppress shot hover)
      // - move_shot: only shots hover (suppress station hover)
      const suppressStationHover = this.activeTool === 'move_shot';
      const suppressShotHover = this.activeTool === 'move_station';

      if (nearestStation && !suppressStationHover) {
        const { sectionId, stationIndex } = nearestStation.properties ?? {};
        if (sectionId !== undefined && stationIndex !== undefined) {
          newHoveredStationId = `${sectionId}:${stationIndex}`;
        }
      }
      // Show shot hover when no station is hovered (or station was suppressed) and shots not suppressed.
      if (!newHoveredStationId && nearestShot && !suppressShotHover) {
        const { sectionId, shotId } = nearestShot.properties ?? {};
        if (sectionId !== undefined && shotId !== undefined) {
          newHoveredShotId = `${sectionId}:${shotId}`;
        }
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
            stationHitArea,
            shotHitArea,
            stationFeatureCount: stationFeatures.length,
            shotFeatureCount: shotFeatures.length,
          });
        }
        this.hoveredStationId = newHoveredStationId;
        this.hoveredShotId = newHoveredShotId;

        // Update cursor - suppress for anchored/immovable features
        let showPointer = Boolean(newHoveredStationId || newHoveredShotId);
        let suppressReason: string | null = null;
        if (showPointer && this.activeTool === 'move_segment' && this.anchoredSectionIds.size > 0) {
          // Check if hovered feature belongs to an anchored section
          const hoveredId = newHoveredStationId || newHoveredShotId;
          if (hoveredId) {
            const sectionId = parseInt(hoveredId.split(':')[0]!, 10);
            if (this.anchoredSectionIds.has(sectionId)) {
              showPointer = false;
              suppressReason = 'anchored_section';
            }
          }
        }
        // Suppress cursor for immovable features in Move Station / Move Shot tools
        if (showPointer && (this.activeTool === 'move_station' || this.activeTool === 'move_shot')) {
          if (newHoveredStationId) {
            const [secStr, idxStr] = newHoveredStationId.split(':');
            const featureId = Number(secStr) * FEATURE_ID_STRIDE + Number(idxStr);
            if (this.isStationImmovable(featureId)) {
              showPointer = false;
              suppressReason = 'immovable_station';
            }
          } else if (newHoveredShotId) {
            const [secStr, idxStr] = newHoveredShotId.split(':');
            const featureId = Number(secStr) * FEATURE_ID_STRIDE + Number(idxStr);
            if (this.isShotImmovable(featureId)) {
              showPointer = false;
              suppressReason = 'immovable_shot';
            }
          }
        }
        // Move Station/Shot tools: use arrow cursor when not over a movable feature
        // (prevents MapLibre's default grab cursor from showing through)
        const useArrowFallback = this.activeTool === 'move_station' || this.activeTool === 'move_shot';
        this.map.getCanvas().style.cursor = showPointer ? 'pointer' : (useArrowFallback ? 'default' : '');
        if (this.shouldLogInteraction() && suppressReason) {
          console.debug('[HitDetection] Hover suppressed', {
            activeTool: this.activeTool,
            suppressReason,
            newHoveredStationId,
            newHoveredShotId,
          });
        }

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
        this.map.getCanvas().style.cursor = '';
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

    // UNIFIED context menu handler - queries both hitbox layers with station priority
    this.map.on('contextmenu', this.handleContextMenu);

    // Touch long-press → context menu (raw touch listeners on canvas)
    const canvas = this.map.getCanvas();
    this.boundTouchStart = (e: TouchEvent) => {
      if (e.touches.length !== 1) return;
      const touch = e.touches[0]!;
      this.touchStartPos = { x: touch.clientX, y: touch.clientY };
      this.touchLongPressTimer = setTimeout(() => {
        this.touchLongPressTimer = null;
        if (!this.touchStartPos) return;
        // Hit-test at touch position
        const rect = canvas.getBoundingClientRect();
        const point: [number, number] = [
          this.touchStartPos.x - rect.left,
          this.touchStartPos.y - rect.top,
        ];
        const fakeEvent = {
          point: { x: point[0], y: point[1] },
          lngLat: this.map.unproject(point),
          originalEvent: new MouseEvent('contextmenu', {
            clientX: this.touchStartPos.x,
            clientY: this.touchStartPos.y,
          }),
        } as maplibregl.MapMouseEvent;
        this.handleContextMenu(fakeEvent);
        // Prevent subsequent click
        this.touchStartPos = null;
      }, 500);
    };
    this.boundTouchMove = (e: TouchEvent) => {
      if (!this.touchStartPos || !this.touchLongPressTimer) return;
      const touch = e.touches[0];
      if (!touch) return;
      const dx = touch.clientX - this.touchStartPos.x;
      const dy = touch.clientY - this.touchStartPos.y;
      if (dx * dx + dy * dy > 100) { // 10px threshold
        clearTimeout(this.touchLongPressTimer);
        this.touchLongPressTimer = null;
        this.touchStartPos = null;
      }
    };
    this.boundTouchEnd = () => {
      if (this.touchLongPressTimer) {
        clearTimeout(this.touchLongPressTimer);
        this.touchLongPressTimer = null;
      }
      this.touchStartPos = null;
    };
    canvas.addEventListener('touchstart', this.boundTouchStart, { passive: false });
    canvas.addEventListener('touchmove', this.boundTouchMove, { passive: false });
    canvas.addEventListener('touchend', this.boundTouchEnd);
    canvas.addEventListener('touchcancel', this.boundTouchEnd);

    // Track user-initiated map navigation (zoom & pan) for telemetry
    this.map.on('zoomend', this.handleZoomEndTelemetry);
    this.map.on('moveend', this.handleMoveEndTelemetry);

    // When the camera crosses the lazy-labels threshold, re-fire update()
    // so the labels source picks up (or sheds) its features. Cheap: the
    // public update() coalesces into the next microtask and only runs the
    // build once even if zoom changes again before flush.
    this.boundZoomEndForLabels = () => {
      if (!hasLiveStyle(this.map)) return;
      const zoom = this.map.getZoom();
      const shouldBuild = this.labelsCurrentlyBuilt
        ? zoom >= SurveyLayer.LABEL_DROP_ZOOM
        : zoom >= SurveyLayer.LABEL_BUILD_ZOOM;
      if (shouldBuild !== this.labelsCurrentlyBuilt
          && (this.cachedSections.length > 0 || this.cachedStations.length > 0)) {
        this.update(
          this.cachedSections,
          this.cachedStations,
          this.cachedLoopClosures,
          this.gpsOrigin,
          this.showDepthGradient,
          this.depthRange,
          this.cachedPrecision,
          this.showIssueSeverity,
          this.cachedWarnings,
          this.cachedMarkers,
        );
      }
    };
    this.map.on('zoomend', this.boundZoomEndForLabels);
  }

  /** Track user-initiated zoom (scroll wheel, pinch, double-click). Skip programmatic zooms. */
  private handleZoomEndTelemetry = (e: maplibregl.MapLibreEvent): void => {
    const orig = e.originalEvent;
    if (!orig) return; // programmatic — already tracked by trigger functions
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
    if (!orig) return; // programmatic — already tracked by trigger functions
    if (orig instanceof WheelEvent) return; // zoom gesture — tracked by zoomend
    if (orig instanceof MouseEvent) {
      trackNav('pan', 'mouse');
    } else if (typeof TouchEvent !== 'undefined' && orig instanceof TouchEvent) {
      trackNav('pan', 'touch');
    }
  };

  /**
   * UNIFIED click handler - queries both hitbox layers and resolves priority.
   * Stations take priority over shots unless the active tool suppresses one type
   * (move_shot suppresses stations, move_station suppresses shots).
   */
  private handleMapClick = (e: MapMouseEvent): void => {
    // Ctrl+click in move tools is for panning, not interacting with features
    if (e.originalEvent.ctrlKey &&
        (this.activeTool === 'move_station' || this.activeTool === 'move_shot' || this.activeTool === 'move_segment')) {
      return;
    }

    // Tool-specific suppression (consistent with hover in processHoverUpdate)
    const suppressStationClick = this.activeTool === 'move_shot';
    const suppressShotClick = this.activeTool === 'move_station';

    // Query stations, shots, and marker layers together
    const allFeatures = this.map.queryRenderedFeatures(e.point, {
      layers: [this.stationsHitboxLayerId, this.shotsHitboxLayerId, this.markersLayerId],
    });
    const stationFeatures = allFeatures.filter((f) => f.layer.id === this.stationsHitboxLayerId);
    const shotFeatures = allFeatures.filter((f) => f.layer.id === this.shotsHitboxLayerId);
    const markerFeatures = allFeatures.filter((f) => f.layer.id === this.markersLayerId);

    const stationHitArea = this.getStationHitArea();
    const shotHitArea = this.getShotHitArea();

    // Stations take priority - resolve nearest station first
    const nearestStation = !suppressStationClick
      ? resolveNearestStation(this.map, e, stationFeatures, stationHitArea)
      : null;
    if (nearestStation) {
      const { sectionId, stationIndex } = nearestStation.properties ?? {};
      if (sectionId !== undefined && stationIndex !== undefined) {
        const ref: StationRef = {
          section_id: sectionId as number,
          station_index: stationIndex as number,
        };
        // Debug logging - only on click, NOT hover (prevents console spam)
        if (this.config?.debug?.map?.log_hit_detection) {
          console.log('[HitDetection] Station click', {
            layer: this.stationsHitboxLayerId,
            featureId: `${sectionId}:${stationIndex}`,
            coordinates: e.lngLat,
            featuresCount: stationFeatures.length,
          });
        }
        this.callbacks.onStationClick?.(ref, e);
        return;
      }
    }

    // Markers take priority over shot lines -- clicking a marker selects its shot
    // and signals the detail panel to open the marker form.
    if (!suppressShotClick && markerFeatures.length > 0) {
      const markerProps = markerFeatures[0]?.properties;
      const mSectionId = markerProps?.['sectionId'];
      const mShotId = markerProps?.['shotId'];
      if (mSectionId !== undefined && mShotId !== undefined) {
        if (this.callbacks.onMarkerClick) {
          this.callbacks.onMarkerClick(mSectionId as number, mShotId as number);
        } else {
          this.callbacks.onShotClick?.(mSectionId as number, mShotId as number, e);
        }
        return;
      }
    }

    // Then shots - resolve nearest shot
    const nearestShot = !suppressShotClick
      ? resolveNearestShot(this.map, e, shotFeatures, shotHitArea)
      : null;
    if (nearestShot) {
      const { sectionId, shotId } = nearestShot.properties ?? {};
      if (sectionId !== undefined && shotId !== undefined) {
        // Debug logging - only on click, NOT hover (prevents console spam)
        if (this.config?.debug?.map?.log_hit_detection) {
          console.log('[HitDetection] Shot click', {
            layer: this.shotsHitboxLayerId,
            featureId: `${sectionId}:${shotId}`,
            coordinates: e.lngLat,
            featuresCount: shotFeatures.length,
          });
        }
        this.callbacks.onShotClick?.(sectionId as number, shotId as number, e);
        return;
      }
    }

    // Empty click - check for hidden features before firing empty callback
    this.handleEmptyClick(e);
  };

  /**
   * UNIFIED context menu handler - queries both hitbox layers and resolves priority.
   * Same pattern as click handler: stations take priority, uses nearest resolution.
   */
  private handleContextMenu = (e: MapMouseEvent): void => {
    // Query both hitbox layers in a single call for better performance
    const allFeatures = this.map.queryRenderedFeatures(e.point, {
      layers: [this.stationsHitboxLayerId, this.shotsHitboxLayerId],
    });
    const stationFeatures = allFeatures.filter((f) => f.layer.id === this.stationsHitboxLayerId);
    const shotFeatures = allFeatures.filter((f) => f.layer.id === this.shotsHitboxLayerId);

    const stationHitArea = this.getStationHitArea();
    const shotHitArea = this.getShotHitArea();

    // Stations take priority
    const nearestStation = resolveNearestStation(this.map, e, stationFeatures, stationHitArea);
    if (nearestStation) {
      const { sectionId, stationIndex } = nearestStation.properties ?? {};
      if (sectionId !== undefined && stationIndex !== undefined) {
        const ref: StationRef = {
          section_id: sectionId as number,
          station_index: stationIndex as number,
        };
        // Debug logging - only on contextmenu, NOT hover (prevents console spam)
        if (this.config?.debug?.map?.log_hit_detection) {
          console.log('[HitDetection] Station context menu', {
            layer: this.stationsHitboxLayerId,
            featureId: `${sectionId}:${stationIndex}`,
            coordinates: e.lngLat,
            featuresCount: stationFeatures.length,
          });
        }
        this.callbacks.onContextMenu?.({ type: 'station', ref }, e);
        return;
      }
    }

    // Then shots
    const nearestShot = resolveNearestShot(this.map, e, shotFeatures, shotHitArea);
    if (nearestShot) {
      const { sectionId, shotId } = nearestShot.properties ?? {};
      if (sectionId !== undefined && shotId !== undefined) {
        // Debug logging - only on contextmenu, NOT hover (prevents console spam)
        if (this.config?.debug?.map?.log_hit_detection) {
          console.log('[HitDetection] Shot context menu', {
            layer: this.shotsHitboxLayerId,
            featureId: `${sectionId}:${shotId}`,
            coordinates: e.lngLat,
            featuresCount: shotFeatures.length,
          });
        }
        this.callbacks.onContextMenu?.({ type: 'shot', sectionId: sectionId as number, shotId: shotId as number }, e);
        return;
      }
    }

    // Empty context menu
    this.callbacks.onContextMenu?.(null, e);
  };

  /**
   * Handle empty click - check for hidden features before firing empty callback.
   * This provides toast feedback when clicking near hidden stations/shots.
   */
  private handleEmptyClick(e: MapMouseEvent): void {
    const HIDDEN_CLICK_THRESHOLD_PX = 15;

    // Check for clicks on hidden stations (stations take priority)
    if (!this.stationsVisible && this.callbacks.onHiddenStationClick) {
      const sourceFeatures = this.map.querySourceFeatures(this.sourceId + '-stations');
      for (const feature of sourceFeatures) {
        if (feature.geometry.type === 'Point') {
          const coords = feature.geometry.coordinates as [number, number];
          const projected = this.map.project(coords);
          const dx = projected.x - e.point.x;
          const dy = projected.y - e.point.y;
          const distance = Math.sqrt(dx * dx + dy * dy);
          if (distance < HIDDEN_CLICK_THRESHOLD_PX) {
            this.callbacks.onHiddenStationClick();
            return;
          }
        }
      }
    }

    // Check for clicks on hidden shots
    if (!this.shotsVisible && this.callbacks.onHiddenShotClick) {
      const sourceFeatures = this.map.querySourceFeatures(this.sourceId + '-shots');
      for (const feature of sourceFeatures) {
        if (feature.geometry.type === 'LineString') {
          const lineCoords = feature.geometry.coordinates as [number, number][];
          for (let i = 0; i < lineCoords.length - 1; i++) {
            const coord1 = lineCoords[i];
            const coord2 = lineCoords[i + 1];
            if (!coord1 || !coord2) continue;
            const p1 = this.map.project(coord1);
            const p2 = this.map.project(coord2);
            const dist = distanceToLineSegment(e.point.x, e.point.y, p1.x, p1.y, p2.x, p2.y);
            if (dist < HIDDEN_CLICK_THRESHOLD_PX) {
              this.callbacks.onHiddenShotClick();
              return;
            }
          }
        }
      }
    }

    // No hidden features nearby, fire empty click
    this.callbacks.onEmptyClick?.(e);
  }

  // Convert "sectionId:index" string ID to numeric featureId for setFeatureState
  private parseFeatureId(id: string | null): number | null {
    if (!id) return null;
    const [sectionId, index] = id.split(':').map(Number);
    if (isNaN(sectionId!) || isNaN(index!)) return null;
    return sectionId! * FEATURE_ID_STRIDE + index!;
  }

  // Collect all feature IDs belonging to a section from a given source.
  // Uses pre-built lookup maps (O(1)) instead of querySourceFeatures (O(N)).
  private getSectionFeatureIds(source: string, sectionIdStr: string): Set<number> {
    const sectionIdNum = Number(sectionIdStr);
    const isShots = source.endsWith('-shots');
    const map = isShots ? this.sectionShotFeatureIds : this.sectionStationFeatureIds;
    return map.get(sectionIdNum) ?? new Set();
  }

  // Get the effective section IDs for hover highlighting.
  // Segment hover (multi-section) takes priority over single-section hover.
  private getEffectiveHoverSectionIds(): string[] {
    if (this.hoveredSegmentSectionIds.length > 0) return this.hoveredSegmentSectionIds;
    if (this.hoveredSectionId) return [this.hoveredSectionId];
    return [];
  }

  // Check if a feature belongs to any section in the current effective hover set
  private featureBelongsToHoveredSections(featureId: number): boolean {
    const ids = this.getEffectiveHoverSectionIds();
    if (ids.length === 0) return false;
    const fSectionId = String(Math.floor(featureId / FEATURE_ID_STRIDE));
    return ids.includes(fSectionId);
  }

  // Get cached section feature IDs for hover, recomputing only when effective section IDs change
  private getCachedHoveredSectionIds(shotsSource: string, stationsSource: string): { shots: Set<number>; stations: Set<number> } {
    const ids = this.getEffectiveHoverSectionIds();
    const key = ids.join(',');
    if (this.cachedHoveredSectionKey !== key) {
      this.cachedHoveredSectionKey = key;
      if (ids.length > 0) {
        const shotIds = new Set<number>();
        const stationIds = new Set<number>();
        for (const secId of ids) {
          for (const id of this.getSectionFeatureIds(shotsSource, secId)) shotIds.add(id);
          for (const id of this.getSectionFeatureIds(stationsSource, secId)) stationIds.add(id);
        }
        this.cachedHoveredSectionShotIds = shotIds;
        this.cachedHoveredSectionStationIds = stationIds;
      } else {
        this.cachedHoveredSectionShotIds = new Set();
        this.cachedHoveredSectionStationIds = new Set();
      }
    }
    return { shots: this.cachedHoveredSectionShotIds, stations: this.cachedHoveredSectionStationIds };
  }

  // Get cached section feature IDs for selection, recomputing only when section ID changes
  private getCachedSelectedSectionIds(shotsSource: string, stationsSource: string): { shots: Set<number>; stations: Set<number> } {
    if (this.cachedSelectedSectionId !== this.selectedSectionId) {
      this.cachedSelectedSectionId = this.selectedSectionId;
      if (this.selectedSectionId) {
        this.cachedSelectedSectionShotIds = this.getSectionFeatureIds(shotsSource, this.selectedSectionId);
        this.cachedSelectedSectionStationIds = this.getSectionFeatureIds(stationsSource, this.selectedSectionId);
      } else {
        this.cachedSelectedSectionShotIds = new Set();
        this.cachedSelectedSectionStationIds = new Set();
      }
    }
    return { shots: this.cachedSelectedSectionShotIds, stations: this.cachedSelectedSectionStationIds };
  }

  // Invalidate section ID caches (called after data updates that reset feature-state)
  private invalidateSectionCaches(): void {
    this.cachedHoveredSectionKey = '';
    this.cachedSelectedSectionId = null;
  }

  // Check if a featureId belongs to a section (derived from encoding: sectionId * FEATURE_ID_STRIDE + index)
  private featureBelongsToSection(featureId: number, sectionIdStr: string | null): boolean {
    if (!sectionIdStr) return false;
    return Math.floor(featureId / FEATURE_ID_STRIDE) === Number(sectionIdStr);
  }

  // Check if a feature belongs to an anchored section (suppresses hover/interaction)
  private isFeatureAnchored(featureId: number): boolean {
    return this.anchoredSectionIds.size > 0 &&
      this.anchoredSectionIds.has(Math.floor(featureId / FEATURE_ID_STRIDE));
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

  // Update visual state using setFeatureState (much faster than setPaintProperty)
  // Only updates the specific features that changed, not the entire layer
  private updateVisualState() {
    const stationsSource = this.sourceId + '-stations';
    const shotsSource = this.sourceId + '-shots';

    // Update station hover state
    const newHoveredStationFeatureId = this.parseFeatureId(this.hoveredStationId);
    if (newHoveredStationFeatureId !== this.prevHoveredStationFeatureId) {
      // Clear previous hover — but restore section/segment hover if feature belongs to hovered sections.
      // Check both the section ID derivation AND the actual section hover set, because featureId
      // encoding may not match sectionId for shots spanning multiple sections.
      if (this.prevHoveredStationFeatureId !== null) {
        const restoreSection = this.featureBelongsToHoveredSections(this.prevHoveredStationFeatureId)
          || this.prevHoveredSectionStationIds.has(this.prevHoveredStationFeatureId);
        this.map.setFeatureState(
          { source: stationsSource, id: this.prevHoveredStationFeatureId },
          { hover: restoreSection }
        );
      }
      // Set new hover (suppress for anchored/immovable features — no interaction feedback)
      if (newHoveredStationFeatureId !== null &&
          !this.isFeatureAnchored(newHoveredStationFeatureId) &&
          !this.isStationImmovable(newHoveredStationFeatureId)) {
        this.map.setFeatureState(
          { source: stationsSource, id: newHoveredStationFeatureId },
          { hover: true }
        );
      }
      this.prevHoveredStationFeatureId = newHoveredStationFeatureId;
    }

    // Update shot hover state
    const newHoveredShotFeatureId = this.parseFeatureId(this.hoveredShotId);
    if (newHoveredShotFeatureId !== this.prevHoveredShotFeatureId) {
      // Clear previous hover — but restore section/segment hover if feature belongs to hovered sections.
      // Check both the section ID derivation AND the actual section hover set, because featureId
      // encoding may not match sectionId for shots spanning multiple sections.
      if (this.prevHoveredShotFeatureId !== null) {
        const restoreSection = this.featureBelongsToHoveredSections(this.prevHoveredShotFeatureId)
          || this.prevHoveredSectionShotIds.has(this.prevHoveredShotFeatureId);
        this.map.setFeatureState(
          { source: shotsSource, id: this.prevHoveredShotFeatureId },
          { hover: restoreSection }
        );
      }
      // Set new hover (suppress for anchored/immovable features — no interaction feedback)
      if (newHoveredShotFeatureId !== null &&
          !this.isFeatureAnchored(newHoveredShotFeatureId) &&
          !this.isShotImmovable(newHoveredShotFeatureId)) {
        this.map.setFeatureState(
          { source: shotsSource, id: newHoveredShotFeatureId },
          { hover: true }
        );
      }
      this.prevHoveredShotFeatureId = newHoveredShotFeatureId;
    }

    // Update station selection state
    const newSelectedStationFeatureId = this.parseFeatureId(this.selectedStationId);
    if (newSelectedStationFeatureId !== this.prevSelectedStationFeatureId) {
      // Clear previous selection — but restore section selection if feature belongs to selected section
      if (this.prevSelectedStationFeatureId !== null) {
        const restoreSection = this.featureBelongsToSection(this.prevSelectedStationFeatureId, this.selectedSectionId);
        this.map.setFeatureState(
          { source: stationsSource, id: this.prevSelectedStationFeatureId },
          { selected: restoreSection }
        );
      }
      // Set new selection
      if (newSelectedStationFeatureId !== null) {
        this.map.setFeatureState(
          { source: stationsSource, id: newSelectedStationFeatureId },
          { selected: true }
        );
      }
      this.prevSelectedStationFeatureId = newSelectedStationFeatureId;
    }

    // Update shot selection state
    const newSelectedShotFeatureId = this.parseFeatureId(this.selectedShotId);
    if (newSelectedShotFeatureId !== this.prevSelectedShotFeatureId) {
      // Clear previous selection — but restore section selection if feature belongs to selected section
      if (this.prevSelectedShotFeatureId !== null) {
        const restoreSection = this.featureBelongsToSection(this.prevSelectedShotFeatureId, this.selectedSectionId);
        this.map.setFeatureState(
          { source: shotsSource, id: this.prevSelectedShotFeatureId },
          { selected: restoreSection }
        );
      }
      // Set new selection
      if (newSelectedShotFeatureId !== null) {
        this.map.setFeatureState(
          { source: shotsSource, id: newSelectedShotFeatureId },
          { selected: true }
        );
      }
      this.prevSelectedShotFeatureId = newSelectedShotFeatureId;
    }

    // --- Section hover (shots + stations) ---
    {
      const { shots: newHoveredShotIds, stations: newHoveredStationIds } =
        this.getCachedHoveredSectionIds(shotsSource, stationsSource);

      // Clear previous section-hovered shots not in new set
      for (const id of this.prevHoveredSectionShotIds) {
        if (!newHoveredShotIds.has(id) && id !== newHoveredShotFeatureId) {
          this.map.setFeatureState({ source: shotsSource, id }, { hover: false });
        }
      }
      // Set new section-hovered shots not in prev set.
      // During move_segment: always re-apply hover on all section shots to ensure visual
      // consistency (individual hover interactions can leave stale feature-state).
      const forceReapply = this.activeTool === 'move_segment';
      for (const id of newHoveredShotIds) {
        if (forceReapply || (!this.prevHoveredSectionShotIds.has(id) && id !== newHoveredShotFeatureId)) {
          this.map.setFeatureState({ source: shotsSource, id }, { hover: true });
        }
      }
      this.prevHoveredSectionShotIds = newHoveredShotIds;

      // Clear previous section-hovered stations not in new set
      for (const id of this.prevHoveredSectionStationIds) {
        if (!newHoveredStationIds.has(id) && id !== newHoveredStationFeatureId) {
          this.map.setFeatureState({ source: stationsSource, id }, { hover: false });
        }
      }
      // Set new section-hovered stations not in prev set
      for (const id of newHoveredStationIds) {
        if (forceReapply || (!this.prevHoveredSectionStationIds.has(id) && id !== newHoveredStationFeatureId)) {
          this.map.setFeatureState({ source: stationsSource, id }, { hover: true });
        }
      }
      this.prevHoveredSectionStationIds = newHoveredStationIds;
    }

    // --- Section selection (shots + stations) ---
    {
      const { shots: newSelectedShotIds, stations: newSelectedStationIds } =
        this.getCachedSelectedSectionIds(shotsSource, stationsSource);

      // Clear previous section-selected shots not in new set
      for (const id of this.prevSelectedSectionShotIds) {
        if (!newSelectedShotIds.has(id) && id !== newSelectedShotFeatureId) {
          this.map.setFeatureState({ source: shotsSource, id }, { selected: false });
        }
      }
      // Set new section-selected shots not in prev set
      for (const id of newSelectedShotIds) {
        if (!this.prevSelectedSectionShotIds.has(id) && id !== newSelectedShotFeatureId) {
          this.map.setFeatureState({ source: shotsSource, id }, { selected: true });
        }
      }
      this.prevSelectedSectionShotIds = newSelectedShotIds;

      // Clear previous section-selected stations not in new set
      for (const id of this.prevSelectedSectionStationIds) {
        if (!newSelectedStationIds.has(id) && id !== newSelectedStationFeatureId) {
          this.map.setFeatureState({ source: stationsSource, id }, { selected: false });
        }
      }
      // Set new section-selected stations not in prev set
      for (const id of newSelectedStationIds) {
        if (!this.prevSelectedSectionStationIds.has(id) && id !== newSelectedStationFeatureId) {
          this.map.setFeatureState({ source: stationsSource, id }, { selected: true });
        }
      }
      this.prevSelectedSectionStationIds = newSelectedStationIds;
    }
  }

  /**
   * Apply the distance-gradient overlay (#34). `enabled` swaps the shots/stations
   * color paint to the distance ramp (or restores section/depth color); `state`
   * carries per-feature `dist` (feature-id → distance, -1 = unreachable).
   * Re-rooting just calls this again — no feature rebuild.
   */
  setDistanceGradient(enabled: boolean, state: DistanceFeatureState | null): void {
    this.distanceMode = enabled;
    this.distanceState = state;
    if (!this.map || !hasLiveStyle(this.map)) return;
    this.applyOverlayPaint();
    this.applyDistanceFeatureState();
  }

  /**
   * Apply the gas reachable-frontier overlay (#38). Mirrors
   * {@link setDistanceGradient}: `enabled` swaps the shots/stations color to the
   * gas ramp (gold near the origin → purple at the turn budget, grey =
   * unreachable); `state` carries per-feature `gasReach` (litres-to-reach).
   * Re-running on an input change just calls this again — no feature rebuild.
   */
  setGasReach(enabled: boolean, state: GasReachFeatureState | null): void {
    this.gasReachMode = enabled;
    this.gasReachState = state;
    if (!this.map || !hasLiveStyle(this.map)) return;
    this.applyOverlayPaint();
    this.applyGasReachFeatureState();
  }

  /**
   * Apply the gas route highlight (#233): the picked penetration route drawn over the survey —
   * swum legs at full opacity, jump legs faded (#243). Has its own GeoJSON source (survives
   * `setData()`); the legs are stored so `applyFeaturesToSources` re-pushes them after a feature
   * rebuild. `[]` clears it.
   */
  setGasRoute(legs: RouteLeg[]): void {
    this.gasRouteLegs = legs;
    this.applyGasRoute();
  }

  /** Build one LineString per route leg (with `isJump` flagged) from `cachedStations` and push them
   *  to the gas-route source. No-op without a live style or the source. */
  private applyGasRoute(): void {
    if (!this.map || !hasLiveStyle(this.map)) return;
    const source = this.map.getSource(this.sourceId + '-gas-route');
    if (!source || source.type !== 'geojson') return;
    const geojsonSource = source as maplibregl.GeoJSONSource;
    // Index the stations once (keyed by section:index) so the per-leg endpoint lookups are O(1) —
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
          properties: { isJump: leg.is_jump },
        },
      ];
    });
    geojsonSource.setData({ type: 'FeatureCollection', features });
  }

  /** The distance-gradient color ramp, shared by the shots line-color and the
   *  stations circle-color so the two can't drift. */
  private distanceRampExpression(): ExpressionSpecification {
    return distanceColorExpression(
      DISTANCE_NEAR,
      DISTANCE_FAR,
      this.distanceState?.max ?? 0,
      this.themeColors.grayLine,
    );
  }

  /** The gas reachable-frontier ramp, shared by the shots line-color and the
   *  stations circle-color so the two can't drift. */
  private gasReachRampExpression(): ExpressionSpecification {
    return gasReachColorExpression(
      GAS_NEAR,
      GAS_FAR,
      this.gasReachState?.max ?? 0,
      this.themeColors.grayLine,
    );
  }

  /** A `measured` feature-state tint case (#26) over the plain per-feature color,
   *  or just the plain color when no path is set. Shared by shots/stations. */
  private measuredTintExpression(measuredIds: Set<number> | null): ExpressionSpecification {
    if (measuredIds && measuredIds.size > 0) {
      return [
        'case',
        ['boolean', ['feature-state', 'measured'], false],
        this.measureTintColor,
        ['get', 'color'],
      ] as ExpressionSpecification;
    }
    return ['get', 'color'] as ExpressionSpecification;
  }

  /** Wrap `base` with a move-comment palette tint (#52): features carrying a
   *  `commentColor` property (the comment's own shot/station) render in that
   *  color; everything else falls through to `base` unchanged. Only called
   *  from the plain/measured branch of shots/stations color - the distance
   *  and gas-reach overlays own color outright while active, so the tint
   *  yields to them rather than fighting for the same paint property. */
  private commentToolTintExpression(base: ExpressionSpecification | string): ExpressionSpecification {
    return ['coalesce', ['get', 'commentColor'], base] as ExpressionSpecification;
  }

  /** The shots layer's `line-color`: distance ramp when active, else the measured
   *  along-line tint (#26), else plain. (The `measurePathLayerId` overlay redraws
   *  the tint on top of the depth gradient; this keeps it correct when the gradient
   *  is off and centralizes the distance-vs-tint precedence.) */
  private shotsLineColor(): ExpressionSpecification {
    if (this.distanceMode) return this.distanceRampExpression();
    if (this.gasReachMode) return this.gasReachRampExpression();
    const base = this.measuredTintExpression(this.measuredPathIds);
    return this.activeTool === 'move_comment' ? this.commentToolTintExpression(base) : base;
  }

  /** The stations layer's `circle-color`: distance ramp when active, else the
   *  measured-path tint for the path's stations (#26), else plain. */
  private stationsCircleColor(): ExpressionSpecification {
    if (this.distanceMode) return this.distanceRampExpression();
    if (this.gasReachMode) return this.gasReachRampExpression();
    const base = this.measuredTintExpression(this.measuredPathStationIds);
    return this.activeTool === 'move_comment' ? this.commentToolTintExpression(base) : base;
  }

  /** Swap shots/stations color to whichever analytical overlay is active (the
   *  distance gradient or the gas reachable-frontier), or restore, and suspend the
   *  depth sub-segment layer so the overlay - or the move-comment palette tint
   *  (#52) - wins cleanly. */
  private applyOverlayPaint(): void {
    if (!this.map) return;
    if (this.map.getLayer(this.shotsLayerId)) {
      this.map.setPaintProperty(this.shotsLayerId, 'line-color', this.shotsLineColor());
    }
    if (this.map.getLayer(this.stationsLayerId)) {
      this.map.setPaintProperty(this.stationsLayerId, 'circle-color', this.stationsCircleColor());
    }
    // Depth's per-segment gradient renders above the main shot line; hide it while
    // distance/gas-reach is active so it can't cover their colors, and while
    // move_comment is active so it can't cover the comment tint (#52 - it draws
    // on the same shots line but ABOVE it, so the coalesce in commentToolTintExpression
    // isn't enough on its own). The measure tint overlay (#26) is intentionally NOT
    // suspended here — the ruler highlight must always sit on top of whatever owns
    // the shot color. When restoring the gradient, match the main shots layer's
    // visibility rather than forcing 'visible' — the user may have Shots hidden
    // (then depth segments must stay hidden too).
    if (this.map.getLayer(this.shotGradientsLayerId)) {
      const shotsVisibility =
        (this.map.getLayoutProperty(this.shotsLayerId, 'visibility') as string | undefined) ??
        'visible';
      this.map.setLayoutProperty(
        this.shotGradientsLayerId,
        'visibility',
        this.distanceMode || this.gasReachMode || this.activeTool === 'move_comment'
          ? 'none'
          : shotsVisibility,
      );
    }
  }

  /** Push per-feature `dist` into feature-state (no-op when the overlay is off). */
  private applyDistanceFeatureState(): void {
    if (!this.map || !this.distanceMode || !this.distanceState) return;
    const shotsSource = this.sourceId + '-shots';
    const stationsSource = this.sourceId + '-stations';
    for (const [id, dist] of this.distanceState.shots) {
      this.map.setFeatureState({ source: shotsSource, id }, { dist });
    }
    for (const [id, dist] of this.distanceState.stations) {
      this.map.setFeatureState({ source: stationsSource, id }, { dist });
    }
  }

  /** Push per-feature `gasReach` into feature-state (no-op when the overlay is off). */
  private applyGasReachFeatureState(): void {
    if (!this.map || !this.gasReachMode || !this.gasReachState) return;
    const shotsSource = this.sourceId + '-shots';
    const stationsSource = this.sourceId + '-stations';
    for (const [id, gasReach] of this.gasReachState.shots) {
      this.map.setFeatureState({ source: shotsSource, id }, { gasReach });
    }
    for (const [id, gasReach] of this.gasReachState.stations) {
      this.map.setFeatureState({ source: stationsSource, id }, { gasReach });
    }
  }

  /** Tint the measured along-cave-line path's shots AND stations (#26) in `color`,
   *  or clear with empty/null sets. Mirrors the distance-gradient overlay: a
   *  `measured` feature-state + a paint `case` (see `shotsLineColor` /
   *  `stationsCircleColor`). The distance gradient still wins on color while it's
   *  active; the tint resurfaces when it turns off. */
  setMeasuredPath(shotIds: Set<number> | null, stationIds: Set<number> | null, color: string): void {
    const shotsSource = this.sourceId + '-shots';
    const stationsSource = this.sourceId + '-stations';
    // Clear any previous tint feature-state.
    if (this.map && this.measuredPathIds) {
      for (const id of this.measuredPathIds) {
        this.map.setFeatureState({ source: shotsSource, id }, { measured: false });
      }
    }
    if (this.map && this.measuredPathStationIds) {
      for (const id of this.measuredPathStationIds) {
        this.map.setFeatureState({ source: stationsSource, id }, { measured: false });
      }
    }
    this.measuredPathIds = shotIds && shotIds.size > 0 ? shotIds : null;
    this.measuredPathStationIds = stationIds && stationIds.size > 0 ? stationIds : null;
    this.measureTintColor = color;
    if (!this.map || !hasLiveStyle(this.map)) return;
    if (this.map.getLayer(this.shotsLayerId)) {
      this.map.setPaintProperty(this.shotsLayerId, 'line-color', this.shotsLineColor());
    }
    if (this.map.getLayer(this.stationsLayerId)) {
      this.map.setPaintProperty(this.stationsLayerId, 'circle-color', this.stationsCircleColor());
    }
    // The on-top tint overlay (#26) reads the same `measured` feature-state; only
    // its color needs updating here.
    if (this.map.getLayer(this.measurePathLayerId)) {
      this.map.setPaintProperty(this.measurePathLayerId, 'line-color', this.measureTintColor);
    }
    this.applyMeasuredPathFeatureState();
  }

  /** Push the `measured` feature-state for the tinted path shots + stations (no-op
   *  when empty). Re-applied after setData() wipes feature-state. */
  private applyMeasuredPathFeatureState(): void {
    if (!this.map) return;
    if (this.measuredPathIds) {
      const shotsSource = this.sourceId + '-shots';
      for (const id of this.measuredPathIds) {
        this.map.setFeatureState({ source: shotsSource, id }, { measured: true });
      }
    }
    if (this.measuredPathStationIds) {
      const stationsSource = this.sourceId + '-stations';
      for (const id of this.measuredPathStationIds) {
        this.map.setFeatureState({ source: stationsSource, id }, { measured: true });
      }
    }
  }

  // Update hitbox layer sizes from config
  private updateHitboxLayers(): void {
    if (!this.map) return;

    const stationHitArea = this.getStationHitArea();
    const shotHitArea = this.getShotHitArea();

    // Update hitbox layer sizes (guard layer existence for safety)
    if (this.map.getLayer(this.stationsHitboxLayerId)) {
      this.map.setPaintProperty(this.stationsHitboxLayerId, 'circle-radius', stationHitArea);
    }
    if (this.map.getLayer(this.shotsHitboxLayerId)) {
      this.map.setPaintProperty(this.shotsHitboxLayerId, 'line-width', shotHitArea);
    }
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

  // Update debug visualization based on config
  // Sets BOTH color AND opacity on all hitbox layers for debug mode
  // CRITICAL: In normal mode, use OPAQUE color (#000) with tiny opacity (0.001)
  // Using transparent color (rgba(0,0,0,0)) would multiply with opacity to give
  // effective alpha of 0, breaking queryRenderedFeatures/hover detection
  private updateDebugVisualization(): void {
    if (!this.map) return;

    const showHitboxes = this.config?.debug?.map?.show_hitboxes ?? false;
    const debugColor = 'rgba(255, 0, 0, 0.3)'; // Semi-transparent red for debug
    const opaqueColor = '#000000'; // OPAQUE color for normal mode (opacity controls visibility)

    // Station hitbox - MUST use opaque color in normal mode for queryRenderedFeatures
    if (this.map.getLayer(this.stationsHitboxLayerId)) {
      this.map.setPaintProperty(
        this.stationsHitboxLayerId,
        'circle-color',
        showHitboxes ? debugColor : opaqueColor
      );
      this.map.setPaintProperty(
        this.stationsHitboxLayerId,
        'circle-opacity',
        showHitboxes ? 1 : 0.001
      );
    }

    // Shot hitbox - MUST use opaque color in normal mode for queryRenderedFeatures
    if (this.map.getLayer(this.shotsHitboxLayerId)) {
      this.map.setPaintProperty(
        this.shotsHitboxLayerId,
        'line-color',
        showHitboxes ? debugColor : opaqueColor
      );
      this.map.setPaintProperty(
        this.shotsHitboxLayerId,
        'line-opacity',
        showHitboxes ? 1 : 0.001
      );
    }

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
  // Uses setFeatureState for instant GPU-side update (same mechanism as hover).
  setSnappedStation(ref: StationRef | null) {
    const newId = ref ? `${ref.section_id}:${ref.station_index}` : null;
    if (this.snappedStationId === newId) return;
    this.snappedStationId = newId;

    // Update snap indicator via setFeatureState (instant per-feature GPU update)
    const stationsSource = this.sourceId + '-stations';
    const newFeatureId = this.parseFeatureId(newId);
    if (newFeatureId !== this.prevSnappedStationFeatureId) {
      if (this.prevSnappedStationFeatureId !== null) {
        this.map.setFeatureState(
          { source: stationsSource, id: this.prevSnappedStationFeatureId },
          { snapped: false }
        );
      }
      if (newFeatureId !== null) {
        this.map.setFeatureState(
          { source: stationsSource, id: newFeatureId },
          { snapped: true }
        );
      }
      this.prevSnappedStationFeatureId = newFeatureId;
    }
  }

  /**
   * Query the nearest station to a screen point using MapLibre's spatial index.
   * O(log N) via queryRenderedFeatures instead of iterating all stations.
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
    const features = this.map.queryRenderedFeatures(bbox, {
      layers: [this.stationsHitboxLayerId],
    });
    if (features.length === 0) return null;

    const fakeEvent = { point: { x: screenX, y: screenY } } as MapMouseEvent;
    const nearest = resolveNearestStation(this.map, fakeEvent, features, threshold);
    if (!nearest) return null;

    // resolveNearestStation skips the distance check for a lone candidate, and the
    // square bbox reaches ~√2·threshold at a corner — enforce the circular snap
    // threshold so a corner station doesn't snap (and, in measure mode, doesn't
    // suppress a closer shot snap) (#26, Codex).
    if (nearest.geometry.type === 'Point') {
      const p = this.map.project(nearest.geometry.coordinates as [number, number]);
      if (Math.hypot(p.x - screenX, p.y - screenY) > threshold) return null;
    }

    const { sectionId, stationIndex } = nearest.properties ?? {};
    if (sectionId === undefined || stationIndex === undefined) return null;

    return {
      section_id: sectionId as number,
      station_index: stationIndex as number,
    };
  }

  /**
   * Nearest shot to a screen point, plus the fractional position `t` ∈ [0,1]
   * along that shot's line (#26 ruler snap-to-shot). The caller interpolates the
   * 3D coords at `t` from the active stations (the 2D shot geometry has no z).
   * Returns null when no shot is within `threshold` pixels.
   *
   * NOTE (known edge case): `t` is derived from the rendered endpoints
   * (`queryRenderedFeatures`), which MapLibre clips at tile boundaries + the
   * source buffer. For a long shot with one station well off-screen, those
   * endpoints can be clip points rather than the true stations, so `t` (and the
   * snap marker) are slightly off for that shot. Short/on-screen shots — the
   * common case — are unaffected (the buffer covers them). A precise fix would
   * require retaining the true per-shot endpoints from setData; deferred as an
   * edge case.
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
    const features = this.map.queryRenderedFeatures(bbox, {
      layers: [this.shotsHitboxLayerId],
    });
    if (features.length === 0) return null;

    const fakeEvent = { point: { x: screenX, y: screenY } } as MapMouseEvent;
    const nearest = resolveNearestShot(this.map, fakeEvent, features, threshold);
    if (!nearest || nearest.geometry.type !== 'LineString') return null;

    const { sectionId, shotId } = nearest.properties ?? {};
    if (sectionId === undefined || shotId === undefined) return null;

    const coords = nearest.geometry.coordinates;
    const c1 = coords[0] as [number, number] | undefined;
    const c2 = coords[coords.length - 1] as [number, number] | undefined;
    if (!c1 || !c2) return null;
    const p1 = this.map.project(c1);
    const p2 = this.map.project(c2);
    // resolveNearestShot skips the distance check for a lone candidate, and the
    // square bbox can include a shot up to ~√2·threshold away at a corner — enforce
    // the (circular) snap threshold against the actual segment so a click that isn't
    // close enough doesn't snap to a shot (#26, Codex).
    if (distanceToLineSegment(screenX, screenY, p1.x, p1.y, p2.x, p2.y) > threshold) return null;
    const t = projectParamOnSegment(screenX, screenY, p1.x, p1.y, p2.x, p2.y);
    const lng = c1[0] + t * (c2[0] - c1[0]);
    const lat = c1[1] + t * (c2[1] - c1[1]);

    return {
      section_id: sectionId as number,
      shot_id: shotId as number,
      t,
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

  // Control shot data labels visibility
  setShowShotLabels(show: boolean) {
    this.map.setLayoutProperty(
      this.shotLabelsLayerId,
      'visibility',
      show ? 'visible' : 'none'
    );
  }

  // Control section name labels visibility
  setShowSectionNames(show: boolean) {
    this.map.setLayoutProperty(
      this.labelsLayerId,
      'visibility',
      show ? 'visible' : 'none'
    );
  }

  // Control station index labels visibility
  setShowStationLabels(show: boolean) {
    this.map.setLayoutProperty(
      this.stationLabelsLayerId,
      'visibility',
      show ? 'visible' : 'none'
    );
  }

  // Control station comments visibility
  setShowStationComments(show: boolean) {
    this.cachedShowStationComments = show;
    // Skip if comment tool override is forcing visibility on
    if (this.commentToolActive) return;
    this.map.setLayoutProperty(
      this.stationCommentsLayerId,
      'visibility',
      show ? 'visible' : 'none'
    );
  }

  // Control shot comments visibility
  setShowShotComments(show: boolean) {
    this.cachedShowShotComments = show;
    // Skip if comment tool override is forcing visibility on
    if (this.commentToolActive) return;
    this.map.setLayoutProperty(
      this.shotCommentsLayerId,
      'visibility',
      show ? 'visible' : 'none'
    );
  }

  // Control stations visibility (also hides loop closures and station hitbox)
  // Note: Highlight layer stays visible - it uses filter to control what renders,
  // allowing snapped/origin stations to appear even when stations layer is hidden
  setShowStations(visible: boolean): void {
    if (!this.map) return;
    this.stationsVisible = visible;
    const visibility = visible ? 'visible' : 'none';
    // Clear hover state FIRST when hiding to prevent any race conditions
    // then set visibility to prevent hover events from triggering while clearing
    if (!visible) {
      this.clearStationHover();
    }
    this.map.setLayoutProperty(this.stationsLayerId, 'visibility', visibility);
    this.map.setLayoutProperty(this.stationsHitboxLayerId, 'visibility', visibility);
    this.map.setLayoutProperty(this.loopClosuresLayerId, 'visibility', visibility);
    // Note: stationsHighlightLayerId intentionally NOT hidden - it uses feature-state
    // to show only snapped/origin stations even when main stations layer is hidden
  }

  // Control shots visibility (also hides shot hitbox)
  setShowShots(visible: boolean): void {
    if (!this.map) return;
    this.shotsVisible = visible;
    const visibility = visible ? 'visible' : 'none';
    // Clear hover state FIRST when hiding to prevent any race conditions
    if (!visible) {
      this.clearShotHover();
    }
    this.map.setLayoutProperty(this.shotsLayerId, 'visibility', visibility);
    this.map.setLayoutProperty(this.measurePathLayerId, 'visibility', visibility);
    this.map.setLayoutProperty(this.shotsHitboxLayerId, 'visibility', visibility);
    // The depth-gradient sublayer's visibility is owned by applyOverlayPaint: it
    // matches the shots layer EXCEPT while distance/gas-reach or the move_comment
    // tint is active, when it must stay hidden. Re-derive it here (rather than
    // forcing it to `visibility` above) so toggling Shots can't re-show the
    // gradient over an active overlay/tint (#52).
    this.applyOverlayPaint();
  }

  // Control reference shots visibility via filter (while keeping regular shots visible)
  // Also hides reference-only stations, their labels, reference shot labels, and
  // section names whose stations are all reference.
  setShowReferenceShots(visible: boolean): void {
    if (!this.map) return;
    this.showReferenceShotsCached = visible;
    const notReference: maplibregl.FilterSpecification = ['!=', ['get', 'isReference'], true];
    const notAllReference: maplibregl.FilterSpecification = ['!=', ['get', 'allShotsReference'], true];
    if (visible) {
      // Restore original filters (label layers have zoom-based density filters)
      this.map.setFilter(this.shotsLayerId, null);
      this.map.setFilter(this.shotGradientsLayerId, null);
      this.map.setFilter(this.measurePathLayerId, null);
      this.map.setFilter(this.shotsHitboxLayerId, null);
      this.map.setFilter(this.stationsLayerId, null);
      this.map.setFilter(this.stationsHitboxLayerId, null);
      this.map.setFilter(this.shotLabelsLayerId, this.shotLabelsBaseFilter);
      this.map.setFilter(this.stationLabelsLayerId, this.stationLabelsBaseFilter);
      this.map.setFilter(this.labelsLayerId, null);
    } else {
      // Hide reference shots and their labels
      this.map.setFilter(this.shotsLayerId, notReference);
      this.map.setFilter(this.shotGradientsLayerId, notReference);
      this.map.setFilter(this.measurePathLayerId, notReference);
      // Also filter the hitbox so a hidden reference shot can't be clicked/snapped
      // (e.g. the ruler snapping to an invisible reference leg) (#26, Codex).
      this.map.setFilter(this.shotsHitboxLayerId, notReference);
      this.map.setFilter(this.shotLabelsLayerId,
        this.shotLabelsBaseFilter
          ? ['all', this.shotLabelsBaseFilter, notReference] as maplibregl.FilterSpecification
          : notReference
      );
      // Hide stations that only connect to reference shots (and their labels)
      this.map.setFilter(this.stationsLayerId, notAllReference);
      this.map.setFilter(this.stationsHitboxLayerId, notAllReference);
      this.map.setFilter(this.stationLabelsLayerId,
        this.stationLabelsBaseFilter
          ? ['all', this.stationLabelsBaseFilter, notAllReference] as maplibregl.FilterSpecification
          : notAllReference
      );
      // Hide section names when all stations in the section are reference
      this.map.setFilter(this.labelsLayerId, ['!=', ['get', 'allStationsReference'], true]);
    }
    // Recompute comment filters (respects both reference-shots and hidden-comment)
    this._applyCommentFilters();
  }

  /**
   * Hide/show the source comment marker during move_comment pickup.
   * Composes with the reference-shots filter so both constraints are respected.
   */
  setHiddenComment(hidden: { type: 'station' | 'shot'; sectionId: number; id: number } | null): void {
    this.hiddenComment = hidden;
    this._applyCommentFilters();
  }

  /**
   * Recompute comment layer filters combining base filter, reference-shot filter,
   * and hidden-comment filter into one expression.
   */
  private _applyCommentFilters(): void {
    if (!this.map) return;
    const commentBase: maplibregl.FilterSpecification = ['!=', ['get', 'comment'], null];
    const notReference: maplibregl.FilterSpecification = ['!=', ['get', 'isReference'], true];
    const notAllReference: maplibregl.FilterSpecification = ['!=', ['get', 'allShotsReference'], true];

    // Station comments filter
    const stationParts: maplibregl.FilterSpecification[] = [commentBase];
    if (!this.showReferenceShotsCached) stationParts.push(notAllReference);
    if (this.hiddenComment?.type === 'station') {
      stationParts.push(['any',
        ['!=', ['get', 'sectionId'], this.hiddenComment.sectionId],
        ['!=', ['get', 'stationIndex'], this.hiddenComment.id],
      ] as maplibregl.FilterSpecification);
    }
    this.map.setFilter(
      this.stationCommentsLayerId,
      stationParts.length === 1 ? stationParts[0]! : ['all', ...stationParts] as maplibregl.FilterSpecification
    );

    // Shot comments filter
    const shotParts: maplibregl.FilterSpecification[] = [commentBase];
    if (!this.showReferenceShotsCached) shotParts.push(notReference);
    if (this.hiddenComment?.type === 'shot') {
      shotParts.push(['any',
        ['!=', ['get', 'sectionId'], this.hiddenComment.sectionId],
        ['!=', ['get', 'shotId'], this.hiddenComment.id],
      ] as maplibregl.FilterSpecification);
    }
    this.map.setFilter(
      this.shotCommentsLayerId,
      shotParts.length === 1 ? shotParts[0]! : ['all', ...shotParts] as maplibregl.FilterSpecification
    );
  }

  /** Force both comment layers visible with overlap priority (comment/move_comment tool) */
  private _enableCommentToolOverride(): void {
    this.commentToolActive = true;
    // Force visible
    this.map.setLayoutProperty(this.stationCommentsLayerId, 'visibility', 'visible');
    this.map.setLayoutProperty(this.shotCommentsLayerId, 'visibility', 'visible');
    // Allow overlap so comments always render on top of other labels
    this.map.setLayoutProperty(this.stationCommentsLayerId, 'text-allow-overlap', true);
    this.map.setLayoutProperty(this.shotCommentsLayerId, 'text-allow-overlap', true);
  }

  /** Restore comment layer visibility and overlap from cached display settings */
  private _disableCommentToolOverride(): void {
    this.commentToolActive = false;
    // Restore visibility from cached display state
    this.map.setLayoutProperty(
      this.stationCommentsLayerId, 'visibility',
      this.cachedShowStationComments ? 'visible' : 'none'
    );
    this.map.setLayoutProperty(
      this.shotCommentsLayerId, 'visibility',
      this.cachedShowShotComments ? 'visible' : 'none'
    );
    // Restore default overlap behavior
    this.map.setLayoutProperty(this.stationCommentsLayerId, 'text-allow-overlap', false);
    this.map.setLayoutProperty(this.shotCommentsLayerId, 'text-allow-overlap', false);
  }

  /** Comment-label `text-color` (#52): palette tint from the feature's
   *  `commentColor` while `move_comment` is the exact active tool, else the
   *  plain theme label color. Shared by both comment layers. Reuses
   *  `commentToolTintExpression` so the shots/stations color tint and the
   *  label text tint can't drift apart. */
  private commentTextColor(): string | ExpressionSpecification {
    return this.activeTool === 'move_comment'
      ? this.commentToolTintExpression(this.themeColors.labelColor)
      : this.themeColors.labelColor;
  }

  /** Set `text-color` from {@link commentTextColor} on both comment layers.
   *  Shared by `applyCommentToolTint` (tool transitions) and `applyThemeColors`
   *  (theme changes) so the two callers can't diverge. */
  private applyCommentTextColor(): void {
    if (!this.map) return;
    const color = this.commentTextColor();
    if (this.map.getLayer(this.stationCommentsLayerId)) {
      this.map.setPaintProperty(this.stationCommentsLayerId, 'text-color', color);
    }
    if (this.map.getLayer(this.shotCommentsLayerId)) {
      this.map.setPaintProperty(this.shotCommentsLayerId, 'text-color', color);
    }
  }

  /** Apply (or restore) the move-comment palette tint on the comment text layers.
   *  A wider halo keeps the vivid palette colors readable over the map while
   *  tinted; restores the normal halo width on exit. Called on tool-transition;
   *  `applyThemeColors` calls `applyCommentTextColor` directly (it does not touch
   *  halo width, only the theme-driven halo color) so a theme change while the
   *  tool is active keeps the tint instead of reverting to the plain label color. */
  private applyCommentToolTint(): void {
    if (!this.map) return;
    this.applyCommentTextColor();
    const haloWidth = this.activeTool === 'move_comment' ? COMMENT_HALO_WIDTH_TINTED : COMMENT_HALO_WIDTH_BASE;
    if (this.map.getLayer(this.stationCommentsLayerId)) {
      this.map.setPaintProperty(this.stationCommentsLayerId, 'text-halo-width', haloWidth);
    }
    if (this.map.getLayer(this.shotCommentsLayerId)) {
      this.map.setPaintProperty(this.shotCommentsLayerId, 'text-halo-width', haloWidth);
    }
  }

  // Set highlighted stations by their string IDs (format: "sectionId:stationIndex")
  setHighlightedStations(stationIds: string[]): void {
    if (!this.map) return;
    const prevIds = this.highlightedStationIds;
    this.highlightedStationIds = stationIds;

    // Update highlight via setFeatureState (instant per-feature GPU update)
    const stationsSource = this.sourceId + '-stations';
    const prevSet = new Set(prevIds);
    const nextSet = new Set(stationIds);

    // Clear snapped state on stations no longer highlighted
    for (const id of prevIds) {
      if (!nextSet.has(id)) {
        const featureId = this.parseFeatureId(id);
        if (featureId !== null) {
          this.map.setFeatureState(
            { source: stationsSource, id: featureId },
            { snapped: false }
          );
        }
      }
    }
    // Set snapped state on newly highlighted stations
    for (const id of stationIds) {
      if (!prevSet.has(id)) {
        const featureId = this.parseFeatureId(id);
        if (featureId !== null) {
          this.map.setFeatureState(
            { source: stationsSource, id: featureId },
            { snapped: true }
          );
        }
      }
    }
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
    this.prevSnappedStationFeatureId = null;
    this.selectedStationId = null;
    this.selectedShotId = null;
    this.selectedSectionId = null;
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
    const stationsSource = this.sourceId + '-stations';
    const shotsSource = this.sourceId + '-shots';

    // Clear previously anchored features
    for (const featureId of this.prevAnchoredFeatureIds) {
      try {
        this.map.setFeatureState({ source: stationsSource, id: featureId }, { anchored: false });
      } catch { /* feature may not exist */ }
      try {
        this.map.setFeatureState({ source: shotsSource, id: featureId }, { anchored: false });
      } catch { /* feature may not exist */ }
    }

    this.anchoredSectionIds = sectionIds;
    const newAnchoredFeatureIds = new Set<number>();

    if (sectionIds.size > 0) {
      // Find all features belonging to anchored sections and set feature-state
      for (const source of [stationsSource, shotsSource]) {
        try {
          const features = this.map.querySourceFeatures(source);
          for (const f of features) {
            if (f.properties?.sectionId != null && sectionIds.has(f.properties.sectionId as number) && f.id != null) {
              const featureId = typeof f.id === 'number' ? f.id : parseInt(String(f.id), 10);
              if (!isNaN(featureId)) {
                this.map.setFeatureState({ source, id: featureId }, { anchored: true });
                newAnchoredFeatureIds.add(featureId);
              }
            }
          }
        } catch { /* source may not be loaded */ }
      }
    }

    this.prevAnchoredFeatureIds = newAnchoredFeatureIds;

    // Update label layer opacity via setPaintProperty (labels use separate sources
    // without promoteId, so feature-state isn't available — use data-driven expressions)
    this.updateLabelAnchoredOpacity(sectionIds);

    // Update loop closure line opacity — gray out closures where both endpoints are anchored
    this.updateClosureAnchoredOpacity(sectionIds);

    // Refresh main layer opacity so anchored sections dim consistently with current set.
    this.updateImmovableLayerOpacity(this.immovableStationFeatureIds, this.immovableShotFeatureIds);
  }

  // Set text-opacity on label layers: 0.25 for features in anchored sections, normal otherwise
  private updateLabelAnchoredOpacity(anchoredIds: Set<number>): void {
    const idsArray = [...anchoredIds];
    const anchoredCheck: maplibregl.ExpressionSpecification =
      ['in', ['get', 'sectionId'], ['literal', idsArray]] as maplibregl.ExpressionSpecification;

    // Zoom-interpolated base opacity for shot/station labels
    const zoomOpacity: maplibregl.ExpressionSpecification = [
      'interpolate', ['linear'], ['zoom'], 14, 0.7, 18, 1,
    ] as maplibregl.ExpressionSpecification;

    if (idsArray.length > 0) {
      // Shot labels: anchored → 0.25, else zoom-interpolated
      this.map.setPaintProperty(this.shotLabelsLayerId, 'text-opacity', [
        'case', anchoredCheck, 0.25, zoomOpacity,
      ] as maplibregl.ExpressionSpecification);
      // Section name labels: anchored → 0.25, else 1
      this.map.setPaintProperty(this.labelsLayerId, 'text-opacity', [
        'case', anchoredCheck, 0.25, 1,
      ] as maplibregl.ExpressionSpecification);
      // Station index labels: anchored → 0.25, else zoom-interpolated
      this.map.setPaintProperty(this.stationLabelsLayerId, 'text-opacity', [
        'case', anchoredCheck, 0.25, zoomOpacity,
      ] as maplibregl.ExpressionSpecification);
    } else {
      // Restore original opacity (no anchored sections)
      this.map.setPaintProperty(this.shotLabelsLayerId, 'text-opacity', zoomOpacity);
      this.map.setPaintProperty(this.labelsLayerId, 'text-opacity', 1);
      this.map.setPaintProperty(this.stationLabelsLayerId, 'text-opacity', zoomOpacity);
    }
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
   * Unlike setAnchoredSections (which uses setFeatureState), this approach is reliable
   * on initial tool activation because it doesn't depend on feature tiling state.
   */
  setImmovableFeatures(stationIds: Set<number>, shotIds: Set<number>): void {
    this.immovableStationFeatureIds = stationIds;
    this.immovableShotFeatureIds = shotIds;

    // Update main layer paint properties (stations + shots)
    this.updateImmovableLayerOpacity(stationIds, shotIds);

    // Update label and closure opacity
    this.updateImmovableLabelOpacity(stationIds, shotIds);
    this.updateImmovableClosureOpacity(stationIds);
  }

  // Set circle-opacity/line-opacity on main layers using data-driven expressions.
  // Uses setPaintProperty (not setFeatureState) for reliable initial-activation dimming.
  private updateImmovableLayerOpacity(stationIds: Set<number>, shotIds: Set<number>): void {
    const anchoredCheck: maplibregl.ExpressionSpecification = this.anchoredSectionIds.size > 0
      ? ['in', ['get', 'sectionId'], ['literal', [...this.anchoredSectionIds]]]
      : (false as unknown as maplibregl.ExpressionSpecification);

    if (stationIds.size > 0 || shotIds.size > 0) {
      // Station circles: dim immovable stations
      if (stationIds.size > 0) {
        const stationIdsArray = [...stationIds];
        const immovableCheck: maplibregl.ExpressionSpecification = [
          'in', ['get', 'featureId'], ['literal', stationIdsArray],
        ] as maplibregl.ExpressionSpecification;
        this.map.setPaintProperty(this.stationsLayerId, 'circle-opacity', [
          'case',
          immovableCheck, 0.25,
          anchoredCheck, 0.25,
          1,
        ] as maplibregl.ExpressionSpecification);
        this.map.setPaintProperty(this.stationsLayerId, 'circle-stroke-opacity', [
          'case',
          immovableCheck, 0.25,
          anchoredCheck, 0.25,
          1,
        ] as maplibregl.ExpressionSpecification);
      } else {
        // Restore default (feature-state only)
        this.map.setPaintProperty(this.stationsLayerId, 'circle-opacity', [
          'case',
          anchoredCheck, 0.25,
          1,
        ] as maplibregl.ExpressionSpecification);
        this.map.setPaintProperty(this.stationsLayerId, 'circle-stroke-opacity', [
          'case',
          anchoredCheck, 0.25,
          1,
        ] as maplibregl.ExpressionSpecification);
      }

      // Shot lines: dim immovable shots
      if (shotIds.size > 0) {
        const shotIdsArray = [...shotIds];
        const immovableCheck: maplibregl.ExpressionSpecification = [
          'in', ['get', 'featureId'], ['literal', shotIdsArray],
        ] as maplibregl.ExpressionSpecification;
        this.map.setPaintProperty(this.shotsLayerId, 'line-opacity', [
          'case',
          immovableCheck, 0.25,
          anchoredCheck, 0.25,
          ['get', 'isReference'], 0.5,
          1,
        ] as maplibregl.ExpressionSpecification);
        this.map.setPaintProperty(this.shotGradientsLayerId, 'line-opacity', [
          'case',
          immovableCheck, 0.25,
          anchoredCheck, 0.25,
          ['get', 'isReference'], 0.5,
          1,
        ] as maplibregl.ExpressionSpecification);
      } else {
        // Restore default (feature-state only)
        this.map.setPaintProperty(this.shotsLayerId, 'line-opacity', [
          'case',
          anchoredCheck, 0.25,
          ['get', 'isReference'], 0.5,
          1,
        ] as maplibregl.ExpressionSpecification);
        this.map.setPaintProperty(this.shotGradientsLayerId, 'line-opacity', [
          'case',
          anchoredCheck, 0.25,
          ['get', 'isReference'], 0.5,
          1,
        ] as maplibregl.ExpressionSpecification);
      }
    } else {
      // No immovable features — restore original paint properties
      this.map.setPaintProperty(this.stationsLayerId, 'circle-opacity', [
        'case',
        anchoredCheck, 0.25,
        1,
      ] as maplibregl.ExpressionSpecification);
      this.map.setPaintProperty(this.stationsLayerId, 'circle-stroke-opacity', [
        'case',
        anchoredCheck, 0.25,
        1,
      ] as maplibregl.ExpressionSpecification);
      this.map.setPaintProperty(this.shotsLayerId, 'line-opacity', [
        'case',
        anchoredCheck, 0.25,
        ['get', 'isReference'], 0.5,
        1,
      ] as maplibregl.ExpressionSpecification);
      this.map.setPaintProperty(this.shotGradientsLayerId, 'line-opacity', [
        'case',
        anchoredCheck, 0.25,
        ['get', 'isReference'], 0.5,
        1,
      ] as maplibregl.ExpressionSpecification);
    }
  }

  // Set text-opacity on label layers: 0.25 for immovable features, normal otherwise.
  // Uses per-feature ID matching (sectionId * FEATURE_ID_STRIDE + stationIndex/shotId) since
  // immovable features are per-station, not per-section.
  private updateImmovableLabelOpacity(
    stationIds: Set<number>,
    shotIds: Set<number>
  ): void {
    const zoomOpacity: maplibregl.ExpressionSpecification = [
      'interpolate', ['linear'], ['zoom'], 14, 0.7, 18, 1,
    ] as maplibregl.ExpressionSpecification;

    if (stationIds.size > 0 || shotIds.size > 0) {
      // Station labels: compute featureId from sectionId * FEATURE_ID_STRIDE + stationIndex
      if (stationIds.size > 0) {
        const stationIdsArray = [...stationIds];
        const stationAnchoredCheck: maplibregl.ExpressionSpecification = [
          'in',
          ['+', ['*', ['get', 'sectionId'], FEATURE_ID_STRIDE], ['get', 'stationIndex']],
          ['literal', stationIdsArray],
        ] as maplibregl.ExpressionSpecification;
        this.map.setPaintProperty(this.stationLabelsLayerId, 'text-opacity', [
          'case', stationAnchoredCheck, 0.25, zoomOpacity,
        ] as maplibregl.ExpressionSpecification);
      } else {
        this.map.setPaintProperty(this.stationLabelsLayerId, 'text-opacity', zoomOpacity);
      }

      // Shot labels: compute featureId from sectionId * FEATURE_ID_STRIDE + shotId
      if (shotIds.size > 0) {
        const shotIdsArray = [...shotIds];
        const shotAnchoredCheck: maplibregl.ExpressionSpecification = [
          'in',
          ['+', ['*', ['get', 'sectionId'], FEATURE_ID_STRIDE], ['get', 'shotId']],
          ['literal', shotIdsArray],
        ] as maplibregl.ExpressionSpecification;
        this.map.setPaintProperty(this.shotLabelsLayerId, 'text-opacity', [
          'case', shotAnchoredCheck, 0.25, zoomOpacity,
        ] as maplibregl.ExpressionSpecification);
      } else {
        this.map.setPaintProperty(this.shotLabelsLayerId, 'text-opacity', zoomOpacity);
      }

      // Section name labels: leave at full opacity (not meaningful to dim per-station)
      this.map.setPaintProperty(this.labelsLayerId, 'text-opacity', 1);
    } else {
      // Restore original opacity
      this.map.setPaintProperty(this.shotLabelsLayerId, 'text-opacity', zoomOpacity);
      this.map.setPaintProperty(this.labelsLayerId, 'text-opacity', 1);
      this.map.setPaintProperty(this.stationLabelsLayerId, 'text-opacity', zoomOpacity);
    }
  }

  // Set line-opacity on loop closures layer: 0.25 when either endpoint is immovable
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

  /**
   * Begin drag preview — hides connected section features from main layers via filter,
   * then creates temporary layers (same sources) that show only those features with a
   * *-translate paint property. Per-frame cost becomes just setPaintProperty (shader
   * uniform update) instead of rebuilding 5 GeoJSON sources.
   */
  beginDragPreview(connectedSectionIds: Set<number>): void {
    if (this.dragPreviewActive) this.endDragPreview();
    if (connectedSectionIds.size === 0) return;

    const idsArray = [...connectedSectionIds];
    const exclusion: maplibregl.FilterSpecification = ['!', ['in', ['get', 'sectionId'], ['literal', idsArray]]];
    const inclusion: maplibregl.FilterSpecification = ['in', ['get', 'sectionId'], ['literal', idsArray]];

    for (const def of SurveyLayer.DRAG_LAYER_DEFS) {
      // Save and compose exclusion filter on main layer
      const existing = this.map.getFilter(def.mainLayerId);
      this.dragSavedFilters.set(def.mainLayerId, existing ?? undefined);
      // Compose exclusion with existing filter. If existing is trivial (null/boolean), use
      // exclusion alone. Cast needed because MapLibre's TS types don't fully cover ['all',...].
      const composed: maplibregl.FilterSpecification = (existing && typeof existing === 'object')
        ? (['all', existing, exclusion] as unknown as maplibregl.FilterSpecification)
        : exclusion;
      this.map.setFilter(def.mainLayerId, composed);

      // Create temp layer from same source, filtered to connected sections only
      const paintBase: Record<string, unknown> = {
        [def.translateProp]: [0, 0] as [number, number],
        [def.translateAnchorProp]: 'viewport',
        // Disable the default 300ms paint transition — makes translate instant
        [`${def.translateProp}-transition`]: { duration: 0, delay: 0 },
      };

      if (def.type === 'line') {
        this.map.addLayer({
          id: def.tempLayerId,
          type: 'line',
          source: def.sourceId,
          filter: inclusion,
          paint: {
            'line-color': ['get', 'color'],
            'line-width': this.getShotSelectionWidth(),
            'line-opacity': 0.85,
            ...paintBase,
          },
        });
      } else if (def.type === 'circle') {
        this.map.addLayer({
          id: def.tempLayerId,
          type: 'circle',
          source: def.sourceId,
          filter: inclusion,
          paint: {
            'circle-color': ['get', 'color'],
            'circle-radius': this.getStationSelectionRadius(),
            'circle-stroke-color': this.themeColors.selectHighlight,
            'circle-stroke-width': 2.5,
            'circle-opacity': 0.85,
            ...paintBase,
          },
        });
      } else if (def.type === 'symbol') {
        // Copy layout properties from main layer so labels keep their position/size
        const vis = this.map.getLayoutProperty(def.mainLayerId, 'visibility') ?? 'visible';
        const textSize = this.map.getLayoutProperty(def.mainLayerId, 'text-size') ?? 11;
        const textOffset = this.map.getLayoutProperty(def.mainLayerId, 'text-offset');
        const textAnchor = this.map.getLayoutProperty(def.mainLayerId, 'text-anchor');
        const layoutProps: Record<string, unknown> = {
          'text-field': def.sourceId.includes('section') ? ['get', 'name'] : ['get', 'label'],
          'text-font': ['Open Sans Regular'],
          'text-size': textSize,
          'text-allow-overlap': true,
          'visibility': vis as 'visible' | 'none',
        };
        if (textOffset != null) layoutProps['text-offset'] = textOffset;
        if (textAnchor != null) layoutProps['text-anchor'] = textAnchor;
        this.map.addLayer({
          id: def.tempLayerId,
          type: 'symbol',
          source: def.sourceId,
          filter: inclusion,
          layout: layoutProps as NonNullable<maplibregl.SymbolLayerSpecification['layout']>,
          paint: {
            'text-color': ['get', 'color'],
            'text-halo-color': this.themeColors.textHalo,
            'text-halo-width': 1.5,
            'text-opacity': 0.85,
            ...paintBase,
          },
        });
      }
    }

    // Loop closures: hide closures that have ANY endpoint in the connected set from
    // the main layer, then create a temp layer for closures with BOTH endpoints in the
    // set (those translate uniformly). Closures bridging dragged/stationary sections are
    // hidden during drag and reappear with correct geometry after commit.
    {
      const closureSourceId = this.sourceId + '-loop-closures';
      const existingFilter = this.map.getFilter(this.loopClosuresLayerId);
      this.dragSavedFilters.set(this.loopClosuresLayerId, existingFilter ?? undefined);

      // Hide closures where EITHER endpoint is in the connected set
      const eitherEndpointExclusion: maplibregl.FilterSpecification = ['!', ['any',
        ['in', ['get', 'sectionIdA'], ['literal', idsArray]],
        ['in', ['get', 'sectionIdB'], ['literal', idsArray]],
      ]] as unknown as maplibregl.FilterSpecification;
      const composedClosure: maplibregl.FilterSpecification = (existingFilter && typeof existingFilter === 'object')
        ? (['all', existingFilter, eitherEndpointExclusion] as unknown as maplibregl.FilterSpecification)
        : eitherEndpointExclusion;
      this.map.setFilter(this.loopClosuresLayerId, composedClosure);

      // Temp layer: closures where BOTH endpoints are in connected set (translate together)
      const bothEndpointInclusion: maplibregl.FilterSpecification = ['all',
        ['in', ['get', 'sectionIdA'], ['literal', idsArray]],
        ['in', ['get', 'sectionIdB'], ['literal', idsArray]],
      ] as unknown as maplibregl.FilterSpecification;

      this.map.addLayer({
        id: this.dragClosuresLayerId,
        type: 'line',
        source: closureSourceId,
        filter: bothEndpointInclusion,
        paint: {
          'line-color': ['get', 'color'],
          'line-width': 3,
          'line-opacity': 0.85,
          'line-dasharray': [6, 4],
          'line-translate': [0, 0] as [number, number],
          'line-translate-anchor': 'viewport',
          'line-translate-transition': { duration: 0, delay: 0 },
        },
      });
    }

    this.dragPreviewActive = true;
  }

  /**
   * Begin station drag preview — finds all co-located stations (merged/closured),
   * hides them and their connected shots from main layers, then creates:
   *   1. Translate-based temp layers for station circles + labels (GPU-efficient)
   *   2. Dynamic GeoJSON sources + layers for connected shots + shot labels
   *      (geometry is rebuilt per-frame since only one endpoint of each shot moves)
   *
   * Returns the list of co-located StationRefs so MapCanvas can batch-move them.
   */
  beginStationDragPreview(sectionId: number, stationIndex: number, additionalSeeds?: StationRef[]): StationRef[] {
    if (this.dragPreviewActive) this.endDragPreview();

    // 1. Find clicked station and all co-located stations via BFS:
    //    - merged_with: always co-located
    //    - closures_with: only when the closure has been aligned
    //    additionalSeeds allows shot drag to seed from both endpoints simultaneously
    const stationByKey = new Map(
      this.cachedStations.map(s => [`${s.station_ref.section_id}:${s.station_ref.station_index}`, s])
    );
    const alignedClosureIds = new Set<bigint>(
      (this.cachedLoopClosures ?? [])
        .filter((closure) => closure.is_aligned)
        .map((closure) => closure.id)
    );
    const clickedKey = `${sectionId}:${stationIndex}`;
    const clickedStation = stationByKey.get(clickedKey);
    if (!clickedStation) return [];

    // Seed BFS from primary station + any additional seeds (e.g., shot's other endpoint)
    const seedKeys = [clickedKey];
    if (additionalSeeds) {
      for (const seed of additionalSeeds) {
        const key = `${seed.section_id}:${seed.station_index}`;
        if (stationByKey.has(key)) seedKeys.push(key);
      }
    }

    const movingRefKeys = new Set<string>();
    const queue: string[] = [...seedKeys];
    while (queue.length > 0) {
      const key = queue.pop()!;
      if (movingRefKeys.has(key)) continue;
      movingRefKeys.add(key);
      const st = stationByKey.get(key);
      if (!st) continue;
      for (const ref of st.merged_with) {
        queue.push(`${ref.section_id}:${ref.station_index}`);
      }
      for (const cr of st.closures_with ?? []) {
        if (!alignedClosureIds.has(cr.closure_id)) continue;
        queue.push(`${cr.station_ref.section_id}:${cr.station_ref.station_index}`);
      }
    }
    const movingStations = this.cachedStations.filter(
      s => movingRefKeys.has(`${s.station_ref.section_id}:${s.station_ref.station_index}`)
    );

    // 2. Build station inclusion/exclusion filters (covers all co-located stations)
    const stationInclusion: maplibregl.FilterSpecification = (movingStations.length === 1
      ? ['all',
          ['==', ['get', 'sectionId'], sectionId],
          ['==', ['get', 'stationIndex'], stationIndex],
        ]
      : ['any',
          ...movingStations.map(s => ['all',
            ['==', ['get', 'sectionId'], s.station_ref.section_id],
            ['==', ['get', 'stationIndex'], s.station_ref.station_index],
          ]),
        ]
    ) as unknown as maplibregl.FilterSpecification;
    const stationExclusion: maplibregl.FilterSpecification =
      ['!', stationInclusion] as unknown as maplibregl.FilterSpecification;

    // 3. Find connected shots for all moving stations
    type ShotInfo = typeof this.stationDragShots extends (infer T)[] | null ? T : never;
    const shotInfos: ShotInfo[] = [];
    const seenShotKeys = new Set<string>();

    for (const movingSt of movingStations) {
      const section = this.cachedSections.find(s => s.id === movingSt.station_ref.section_id);
      if (!section) continue;

      const sectionStations = this.cachedStations
        .filter(s => s.station_ref.section_id === section.id)
        .sort((a, b) => a.station_ref.station_index - b.station_ref.station_index);

      for (let i = 0; i < section.shots.length; i++) {
        const fromSt = sectionStations[i];
        const toSt = sectionStations[i + 1];
        if (!fromSt || !toSt) continue;

        const fromKey = `${fromSt.station_ref.section_id}:${fromSt.station_ref.station_index}`;
        const toKey = `${toSt.station_ref.section_id}:${toSt.station_ref.station_index}`;
        const fromIsMoving = movingRefKeys.has(fromKey);
        const toIsMoving = movingRefKeys.has(toKey);
        if (!fromIsMoving && !toIsMoving) continue;

        const shot = section.shots[i];
        if (!shot) continue;
        const shotKey = `${section.id}:${shot.id}`;
        if (seenShotKeys.has(shotKey)) continue;
        seenShotKeys.add(shotKey);

        const fromCoord = metersToMapCoords(
          fromSt.coordinates.x, fromSt.coordinates.y, this.gpsOrigin
        ) as [number, number];
        const toCoord = metersToMapCoords(
          toSt.coordinates.x, toSt.coordinates.y, this.gpsOrigin
        ) as [number, number];

        shotInfos.push({
          properties: {
            sectionId: section.id,
            shotId: shot.id,
            color: section.color,
            isReference: shot.is_reference,
          },
          fromCoord,
          toCoord,
          movingEnd: fromIsMoving && toIsMoving ? 'both' : fromIsMoving ? 'from' : 'to',
          shotLabelProperties: {
            label: `${formatFixed(shot.azimuth_in, undefined, this.cachedPrecision?.azimuth ?? 0)}° ${formatFixed(shot.length, undefined, this.cachedPrecision?.length ?? 1)}m`,
            sectionId: section.id,
            shotId: shot.id,
            color: section.color,
          },
        });
      }
    }

    // 4. Create translate-based temp layers for stations + station labels
    const stationDefs = SurveyLayer.DRAG_LAYER_DEFS.filter(
      d => d.mainLayerId === 'survey-stations' || d.mainLayerId === 'survey-station-labels'
    );

    for (const def of stationDefs) {
      const existing = this.map.getFilter(def.mainLayerId);
      this.dragSavedFilters.set(def.mainLayerId, existing ?? undefined);
      const composed: maplibregl.FilterSpecification = (existing && typeof existing === 'object')
        ? (['all', existing, stationExclusion] as unknown as maplibregl.FilterSpecification)
        : stationExclusion;
      this.map.setFilter(def.mainLayerId, composed);

      const paintBase: Record<string, unknown> = {
        [def.translateProp]: [0, 0] as [number, number],
        [def.translateAnchorProp]: 'viewport',
        [`${def.translateProp}-transition`]: { duration: 0, delay: 0 },
      };

      if (def.type === 'circle') {
        this.map.addLayer({
          id: def.tempLayerId,
          type: 'circle',
          source: def.sourceId,
          filter: stationInclusion,
          paint: {
            'circle-color': ['get', 'color'],
            'circle-radius': this.getStationSelectionRadius(),
            'circle-stroke-color': this.themeColors.selectHighlight,
            'circle-stroke-width': 2.5,
            'circle-opacity': 0.85,
            ...paintBase,
          },
        });
      } else if (def.type === 'symbol') {
        const vis = this.map.getLayoutProperty(def.mainLayerId, 'visibility') ?? 'visible';
        const textSize = this.map.getLayoutProperty(def.mainLayerId, 'text-size') ?? 11;
        const textOffset = this.map.getLayoutProperty(def.mainLayerId, 'text-offset');
        const textAnchor = this.map.getLayoutProperty(def.mainLayerId, 'text-anchor');
        const layoutProps: Record<string, unknown> = {
          'text-field': ['get', 'label'],
          'text-font': ['Open Sans Regular'],
          'text-size': textSize,
          'text-allow-overlap': true,
          'visibility': vis as 'visible' | 'none',
        };
        if (textOffset != null) layoutProps['text-offset'] = textOffset;
        if (textAnchor != null) layoutProps['text-anchor'] = textAnchor;
        this.map.addLayer({
          id: def.tempLayerId,
          type: 'symbol',
          source: def.sourceId,
          filter: stationInclusion,
          layout: layoutProps as NonNullable<maplibregl.SymbolLayerSpecification['layout']>,
          paint: {
            'text-color': ['get', 'color'],
            'text-halo-color': this.themeColors.textHalo,
            'text-halo-width': 1.5,
            'text-opacity': 0.85,
            ...paintBase,
          },
        });
      }
    }

    // 5. Create dynamic sources + layers for connected shots + shot labels
    if (shotInfos.length > 0) {
      // Build shot exclusion filter and apply to main shot + shot-label layers
      const firstShot = shotInfos[0]!;
      const shotInclusion: maplibregl.FilterSpecification = (shotInfos.length === 1
        ? ['all',
            ['==', ['get', 'sectionId'], firstShot.properties.sectionId],
            ['==', ['get', 'shotId'], firstShot.properties.shotId],
          ]
        : ['any',
            ...shotInfos.map(s => ['all',
              ['==', ['get', 'sectionId'], s.properties.sectionId],
              ['==', ['get', 'shotId'], s.properties.shotId],
            ]),
          ]
      ) as unknown as maplibregl.FilterSpecification;
      const shotExclusion: maplibregl.FilterSpecification =
        ['!', shotInclusion] as unknown as maplibregl.FilterSpecification;

      for (const layerId of ['survey-shots', 'survey-shot-gradients', 'survey-shot-labels']) {
        const existing = this.map.getFilter(layerId);
        this.dragSavedFilters.set(layerId, existing ?? undefined);
        const composed: maplibregl.FilterSpecification = (existing && typeof existing === 'object')
          ? (['all', existing, shotExclusion] as unknown as maplibregl.FilterSpecification)
          : shotExclusion;
        this.map.setFilter(layerId, composed);
      }

      // Build initial GeoJSON for shots
      const shotFeatures: GeoJSON.Feature[] = shotInfos.map(s => ({
        type: 'Feature' as const,
        geometry: { type: 'LineString' as const, coordinates: [s.fromCoord, s.toCoord] },
        properties: s.properties,
      }));

      this.map.addSource(SurveyLayer.STATION_DRAG_SHOT_SOURCE, {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: shotFeatures },
      });
      this.map.addLayer({
        id: SurveyLayer.STATION_DRAG_SHOT_LAYER,
        type: 'line',
        source: SurveyLayer.STATION_DRAG_SHOT_SOURCE,
        paint: {
          'line-color': ['get', 'color'],
          'line-width': this.getShotSelectionWidth(),
          'line-opacity': 0.85,
        },
      });

      // Build initial GeoJSON for shot labels
      const shotLabelFeatures: GeoJSON.Feature[] = shotInfos.map(s => ({
        type: 'Feature' as const,
        geometry: {
          type: 'Point' as const,
          coordinates: [(s.fromCoord[0] + s.toCoord[0]) / 2, (s.fromCoord[1] + s.toCoord[1]) / 2],
        },
        properties: s.shotLabelProperties,
      }));

      this.map.addSource(SurveyLayer.STATION_DRAG_SHOT_LABEL_SOURCE, {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: shotLabelFeatures },
      });
      const shotLabelVis = this.map.getLayoutProperty('survey-shot-labels', 'visibility') ?? 'visible';
      const shotLabelSize = this.map.getLayoutProperty('survey-shot-labels', 'text-size') ?? 11;
      this.map.addLayer({
        id: SurveyLayer.STATION_DRAG_SHOT_LABEL_LAYER,
        type: 'symbol',
        source: SurveyLayer.STATION_DRAG_SHOT_LABEL_SOURCE,
        layout: {
          'text-field': ['get', 'label'],
          'text-font': ['Open Sans Regular'],
          'text-size': shotLabelSize as number,
          'text-allow-overlap': true,
          'visibility': shotLabelVis as 'visible' | 'none',
        },
        paint: {
          'text-color': ['get', 'color'],
          'text-halo-color': this.themeColors.textHalo,
          'text-halo-width': 1.5,
          'text-opacity': 0.85,
        },
      });
    }

    this.stationDragShots = shotInfos.length > 0 ? shotInfos : null;

    // 6. Find closure indicator lines with at least one endpoint in the moving set
    type ClosureInfo = NonNullable<typeof this.stationDragClosures>[number];
    const closureInfos: ClosureInfo[] = [];
    if (this.cachedLoopClosures && this.cachedLoopClosures.length > 0) {
      for (const lc of this.cachedLoopClosures) {
        const keyA = `${lc.station_a.section_id}:${lc.station_a.station_index}`;
        const keyB = `${lc.station_b.section_id}:${lc.station_b.station_index}`;
        const aMoving = movingRefKeys.has(keyA);
        const bMoving = movingRefKeys.has(keyB);
        if (!aMoving && !bMoving) continue;

        const stA = stationByKey.get(keyA);
        const stB = stationByKey.get(keyB);
        if (!stA || !stB) continue;

        const fromCoord = metersToMapCoords(
          stA.coordinates.x, stA.coordinates.y, this.gpsOrigin
        ) as [number, number];
        const toCoord = metersToMapCoords(
          stB.coordinates.x, stB.coordinates.y, this.gpsOrigin
        ) as [number, number];

        closureInfos.push({
          properties: {
            stationA: keyA,
            stationB: keyB,
            sectionIdA: lc.station_a.section_id,
            sectionIdB: lc.station_b.section_id,
            errorPercentage: lc.error_percentage,
            misclosureDistance: lc.misclosure_distance,
            pathLength: lc.path_length,
            color: getErrorColor(lc.error_percentage),
          },
          fromCoord,
          toCoord,
          movingEnd: aMoving && bMoving ? 'both' : aMoving ? 'from' : 'to',
        });
      }
    }

    if (closureInfos.length > 0) {
      // Build exclusion filter for closures with ANY endpoint in moving set
      const closureExclusion: maplibregl.FilterSpecification = ['!', ['any',
        ...closureInfos.map(c => ['all',
          ['==', ['get', 'stationA'], c.properties.stationA],
          ['==', ['get', 'stationB'], c.properties.stationB],
        ]),
      ]] as unknown as maplibregl.FilterSpecification;

      const existingClosureFilter = this.map.getFilter(this.loopClosuresLayerId);
      this.dragSavedFilters.set(this.loopClosuresLayerId, existingClosureFilter ?? undefined);
      const composedClosure: maplibregl.FilterSpecification = (existingClosureFilter && typeof existingClosureFilter === 'object')
        ? (['all', existingClosureFilter, closureExclusion] as unknown as maplibregl.FilterSpecification)
        : closureExclusion;
      this.map.setFilter(this.loopClosuresLayerId, composedClosure);

      // Create dynamic GeoJSON source + layer for closure lines
      const closureFeatures: GeoJSON.Feature[] = closureInfos.map(c => ({
        type: 'Feature' as const,
        geometry: { type: 'LineString' as const, coordinates: [c.fromCoord, c.toCoord] },
        properties: c.properties,
      }));

      this.map.addSource(SurveyLayer.STATION_DRAG_CLOSURE_SOURCE, {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: closureFeatures },
      });
      this.map.addLayer({
        id: SurveyLayer.STATION_DRAG_CLOSURE_LAYER,
        type: 'line',
        source: SurveyLayer.STATION_DRAG_CLOSURE_SOURCE,
        paint: {
          'line-color': ['get', 'color'],
          'line-width': 3,
          'line-opacity': 0.85,
          'line-dasharray': [6, 4],
        },
      });
    }

    this.stationDragClosures = closureInfos.length > 0 ? closureInfos : null;
    this.dragPreviewActive = true;
    return movingStations.map(s => s.station_ref);
  }

  /**
   * Update station drag preview — translates station circles/labels AND
   * rebuilds connected shot geometries so they stretch to follow the cursor.
   * Shot geometry rebuild is cheap (typically 1-5 features).
   */
  updateStationDragPreview(dx: number, dy: number): void {
    if (!this.dragPreviewActive) return;

    // Update station circles + labels via translate (GPU shader uniform)
    const offset: [number, number] = [dx, dy];
    for (const def of SurveyLayer.DRAG_LAYER_DEFS) {
      if (this.map.getLayer(def.tempLayerId)) {
        this.map.setPaintProperty(def.tempLayerId, def.translateProp, offset);
      }
    }

    // Rebuild connected shot geometries
    if (!this.stationDragShots) return;

    const shotFeatures: GeoJSON.Feature[] = [];
    const shotLabelFeatures: GeoJSON.Feature[] = [];

    for (const s of this.stationDragShots) {
      let fromLng = s.fromCoord[0], fromLat = s.fromCoord[1];
      let toLng = s.toCoord[0], toLat = s.toCoord[1];

      if (s.movingEnd === 'from' || s.movingEnd === 'both') {
        const p = this.map.project([fromLng, fromLat]);
        const np = this.map.unproject([p.x + dx, p.y + dy]);
        fromLng = np.lng;
        fromLat = np.lat;
      }
      if (s.movingEnd === 'to' || s.movingEnd === 'both') {
        const p = this.map.project([toLng, toLat]);
        const np = this.map.unproject([p.x + dx, p.y + dy]);
        toLng = np.lng;
        toLat = np.lat;
      }

      shotFeatures.push({
        type: 'Feature',
        geometry: { type: 'LineString', coordinates: [[fromLng, fromLat], [toLng, toLat]] },
        properties: s.properties,
      });

      shotLabelFeatures.push({
        type: 'Feature',
        geometry: {
          type: 'Point',
          coordinates: [(fromLng + toLng) / 2, (fromLat + toLat) / 2],
        },
        properties: s.shotLabelProperties,
      });
    }

    const shotSource = this.map.getSource(SurveyLayer.STATION_DRAG_SHOT_SOURCE);
    if (shotSource && 'setData' in shotSource) {
      (shotSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection', features: shotFeatures,
      });
    }
    const shotLabelSource = this.map.getSource(SurveyLayer.STATION_DRAG_SHOT_LABEL_SOURCE);
    if (shotLabelSource && 'setData' in shotLabelSource) {
      (shotLabelSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection', features: shotLabelFeatures,
      });
    }

    // Rebuild closure indicator line geometries
    if (this.stationDragClosures) {
      const closureFeatures: GeoJSON.Feature[] = [];
      for (const c of this.stationDragClosures) {
        let fromLng = c.fromCoord[0], fromLat = c.fromCoord[1];
        let toLng = c.toCoord[0], toLat = c.toCoord[1];

        if (c.movingEnd === 'from' || c.movingEnd === 'both') {
          const p = this.map.project([fromLng, fromLat]);
          const np = this.map.unproject([p.x + dx, p.y + dy]);
          fromLng = np.lng;
          fromLat = np.lat;
        }
        if (c.movingEnd === 'to' || c.movingEnd === 'both') {
          const p = this.map.project([toLng, toLat]);
          const np = this.map.unproject([p.x + dx, p.y + dy]);
          toLng = np.lng;
          toLat = np.lat;
        }

        closureFeatures.push({
          type: 'Feature',
          geometry: { type: 'LineString', coordinates: [[fromLng, fromLat], [toLng, toLat]] },
          properties: c.properties,
        });
      }

      const closureSource = this.map.getSource(SurveyLayer.STATION_DRAG_CLOSURE_SOURCE);
      if (closureSource && 'setData' in closureSource) {
        (closureSource as maplibregl.GeoJSONSource).setData({
          type: 'FeatureCollection', features: closureFeatures,
        });
      }
    }
  }

  /**
   * Update drag preview offset — just updates *-translate paint properties.
   * This is a shader uniform update (essentially free, no GeoJSON rebuild).
   * Guards against missing layers so it works for both full segment drag
   * (5 temp layers) and single station drag (2 temp layers).
   */
  updateDragPreview(dx: number, dy: number): void {
    if (!this.dragPreviewActive) return;
    const offset: [number, number] = [dx, dy];
    for (const def of SurveyLayer.DRAG_LAYER_DEFS) {
      if (this.map.getLayer(def.tempLayerId)) {
        this.map.setPaintProperty(def.tempLayerId, def.translateProp, offset);
      }
    }
    // Update closure drag layer
    if (this.map.getLayer(this.dragClosuresLayerId)) {
      this.map.setPaintProperty(this.dragClosuresLayerId, 'line-translate', offset);
    }
  }

  /**
   * Commit drag preview — removes temp layers but keeps exclusion filters on main
   * layers so connected sections stay hidden. Call this before the backend commit,
   * then call endDragPreview() after React re-renders with new positions.
   * This prevents a blink where features briefly appear at newPosition + dragOffset.
   */
  commitDragPreview(): void {
    if (!this.dragPreviewActive) return;
    for (const def of SurveyLayer.DRAG_LAYER_DEFS) {
      if (this.map.getLayer(def.tempLayerId)) {
        this.map.removeLayer(def.tempLayerId);
      }
    }
    if (this.map.getLayer(this.dragClosuresLayerId)) {
      this.map.removeLayer(this.dragClosuresLayerId);
    }
    // Station drag dynamic layers + sources
    this.removeStationDragLayers();
  }

  /**
   * End drag preview — removes temporary layers (if still present) and restores
   * original filters so connected sections are visible again.
   */
  endDragPreview(): void {
    if (!this.dragPreviewActive) return;

    for (const def of SurveyLayer.DRAG_LAYER_DEFS) {
      // Remove temp layer if commitDragPreview wasn't called first
      if (this.map.getLayer(def.tempLayerId)) {
        this.map.removeLayer(def.tempLayerId);
      }

      // Restore original filter on main layer — only if we saved one
      // (station drag only modifies 2 of 5 layers)
      if (this.dragSavedFilters.has(def.mainLayerId)) {
        const saved = this.dragSavedFilters.get(def.mainLayerId);
        this.map.setFilter(def.mainLayerId, saved ?? null);
      }
    }

    // Remove closure temp layer and restore filter
    if (this.map.getLayer(this.dragClosuresLayerId)) {
      this.map.removeLayer(this.dragClosuresLayerId);
    }
    if (this.dragSavedFilters.has(this.loopClosuresLayerId)) {
      const savedClosureFilter = this.dragSavedFilters.get(this.loopClosuresLayerId);
      this.map.setFilter(this.loopClosuresLayerId, savedClosureFilter ?? null);
    }

    // Station drag dynamic layers + sources
    this.removeStationDragLayers();

    // Restore shot/shot-label filters if saved (station drag modifies these)
    for (const layerId of ['survey-shots', 'survey-shot-labels']) {
      if (this.dragSavedFilters.has(layerId)) {
        const saved = this.dragSavedFilters.get(layerId);
        this.map.setFilter(layerId, saved ?? null);
      }
    }

    this.stationDragShots = null;
    this.stationDragClosures = null;
    this.dragSavedFilters.clear();
    this.dragPreviewActive = false;
  }

  /** Remove station drag dynamic layers and sources (idempotent). */
  private removeStationDragLayers(): void {
    for (const layerId of [
      SurveyLayer.STATION_DRAG_SHOT_LAYER,
      SurveyLayer.STATION_DRAG_SHOT_LABEL_LAYER,
      SurveyLayer.STATION_DRAG_CLOSURE_LAYER,
    ]) {
      if (this.map.getLayer(layerId)) this.map.removeLayer(layerId);
    }
    for (const sourceId of [
      SurveyLayer.STATION_DRAG_SHOT_SOURCE,
      SurveyLayer.STATION_DRAG_SHOT_LABEL_SOURCE,
      SurveyLayer.STATION_DRAG_CLOSURE_SOURCE,
    ]) {
      if (this.map.getSource(sourceId)) this.map.removeSource(sourceId);
    }
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
    warnings: ProjectWarning[] = [],
    markers?: Marker[],
  ) {
    // Overwrite the pending args — last-call-wins is correct because the
    // builder is deterministic in its inputs; coalescing equivalent calls
    // is the whole point.
    this.pendingUpdateArgs = {
      sections,
      stations,
      loopClosures,
      markers,
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
    // callback to land in the same frame — they're restoring filters
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
        args.markers,
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
      // Off-thread build for the normal (non-drag-commit) path. Falls
      // back to in-line build inside the worker client if Worker can't
      // be constructed (test envs).
      void this.flushUpdateViaWorker(
        args.sections,
        args.stations,
        args.loopClosures,
        args.gpsOrigin,
        args.showDepthGradient,
        args.depthRange,
        args.precision,
        args.showIssueSeverity,
        args.warnings,
        args.markers,
      );
    });
  }

  /**
   * Async path: feed inputs to the features worker, await the result,
   * then apply via the shared `applyFeaturesToSources` helper. The
   * synchronous `flushUpdate` is kept for drag-commit / sync callers.
   */
  private async flushUpdateViaWorker(
    sections: Section[],
    stations: Station[],
    loopClosures: LoopClosure[] | undefined,
    gpsOrigin: GpsOrigin | null,
    showDepthGradient: boolean,
    depthRange: DepthRange,
    precision: DecimalPrecision | undefined,
    showIssueSeverity: boolean = false,
    warnings: ProjectWarning[] = [],
    markers: Marker[] | undefined = undefined,
  ): Promise<void> {
    if (!hasLiveStyle(this.map)) return;
    this.coalescedUpdateCallsSinceLastFlush = 0;
    this.gpsOrigin = gpsOrigin;
    this.showDepthGradient = showDepthGradient;
    this.depthRange = normalizeDepthRange(depthRange);
    this.showIssueSeverity = showIssueSeverity;
    this.cachedWarnings = warnings;
    this.cachedSections = sections;
    this.cachedStations = stations;
    this.cachedLoopClosures = loopClosures;
    this.cachedMarkers = markers;
    if (precision !== undefined) this.cachedPrecision = precision;

    const zoom = hasLiveStyle(this.map) ? this.map.getZoom() : 0;
    const buildLabels = this.labelsCurrentlyBuilt
      ? zoom >= SurveyLayer.LABEL_DROP_ZOOM
      : zoom >= SurveyLayer.LABEL_BUILD_ZOOM;
    this.labelsCurrentlyBuilt = buildLabels;

    // Tag the call so a later flush invalidates an in-flight build —
    // protects against a fast cascade where the user fires update() again
    // while the worker is still building from the previous args.
    const myToken = ++this.lastUpdateToken;

    // Convert to SurveyData once before posting to the worker. Doing
    // this here (rather than at the call boundary in MapCanvas) lets
    // the existing cached `sections`/`stations` continue to feed
    // drag-preview / theme-rebuild paths inside SurveyLayer that
    // haven't been migrated yet. The conversion is bounded by N (one
    // pass over shots + stations); for the 75k-shot project it's
    // ~10 ms on a single tight loop and the result is a typed-array
    // bundle that structured-clones to the worker far cheaper than
    // 75k+89k JS objects ever would.
    const surveyData = selectActiveSurveyData(
      useProjectStore.getState(),
      useUIStore.getState().geometryMode
    ) ?? surveyDataFromLegacy(sections, stations);

    // Issue-severity arrays are computed against this exact surveyData so the
    // per-shot/per-station indices align with what the builder iterates.
    const issueSeverity = showIssueSeverity
      ? computeIssueSeverity(surveyData, warnings)
      : undefined;
    const buildOptions = { buildLabels, showIssueSeverity, issueSeverity };

    const tBuildStart = performance.now();
    let features: ReturnType<typeof buildSurveyFeatures>;
    try {
      features = await buildSurveyFeaturesAsync({
        surveyData,
        loopClosures,
        markers,
        gpsOrigin: this.gpsOrigin,
        showDepthGradient: this.showDepthGradient,
        depthRange: this.depthRange,
        themeColors: this.themeColors,
        precision: this.cachedPrecision,
        options: buildOptions,
      });
    } catch (e) {
      console.error('[SurveyLayer] feature worker failed, falling back to inline build:', e);
      // Inline fallback so the survey still renders.
      features = buildSurveyFeatures(
        surveyData,
        loopClosures,
        this.gpsOrigin,
        this.showDepthGradient,
        this.depthRange,
        this.themeColors,
        this.cachedPrecision,
        buildOptions,
        markers,
      );
    }
    recordPerfTiming('buildFeatures (async)', performance.now() - tBuildStart);

    // Bail if a newer update has already taken over.
    if (myToken !== this.lastUpdateToken) return;
    if (!hasLiveStyle(this.map)) return;
    this.applyFeaturesToSources(features);
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
    markers?: Marker[],
  ): void {
    // Cancel any pending coalesced flush so we don't re-run with stale args.
    this.updateScheduled = false;
    this.pendingUpdateArgs = null;
    // Bump the token so any in-flight `flushUpdateViaWorker` (started by
    // an earlier update before this sync flush kicked in) discards its
    // result via the `myToken !== this.lastUpdateToken` guard.  Without
    // this the older worker could land its stale features on top of the
    // freshly-applied sync data — most visibly during a drag commit
    // that arrives between the user's last zoom rebuild and the worker
    // finishing.
    this.lastUpdateToken += 1;
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
      markers,
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
    markers: Marker[] | undefined = undefined,
  ): void {
    // Early bail when the map is gone or its style has been torn down — the
    // coalescer's queueMicrotask can race with MapCanvas unmount → map.remove(),
    // and setData on a removed source throws. We have to check this BEFORE
    // hitting getSource/setData below.
    if (!hasLiveStyle(this.map)) return;

    // Reset the call counter so the probe can show calls/tick coherently.
    this.coalescedUpdateCallsSinceLastFlush = 0;
    // Store gpsOrigin for coordinate transforms
    this.gpsOrigin = gpsOrigin;
    // Store depth mode parameters for use in feature color calculation
    this.showDepthGradient = showDepthGradient;
    this.depthRange = normalizeDepthRange(depthRange);
    this.showIssueSeverity = showIssueSeverity;
    this.cachedWarnings = warnings;

    // Cache input data for feature rebuild on theme change
    this.cachedSections = sections;
    this.cachedStations = stations;
    this.cachedLoopClosures = loopClosures;
    this.cachedMarkers = markers;
    if (precision !== undefined) this.cachedPrecision = precision;

    // Decide whether labels should be materialized based on current zoom.
    // Hysteresis: build past LABEL_BUILD_ZOOM, drop only when zoomed out
    // below LABEL_DROP_ZOOM — prevents thrashing at the threshold.
    const zoom = hasLiveStyle(this.map) ? this.map.getZoom() : 0;
    const buildLabels = this.labelsCurrentlyBuilt
      ? zoom >= SurveyLayer.LABEL_DROP_ZOOM
      : zoom >= SurveyLayer.LABEL_BUILD_ZOOM;
    this.labelsCurrentlyBuilt = buildLabels;

    // Synchronous (drag-commit) path: convert to SurveyData inline
    // before building. See the async counterpart for the rationale.
    const surveyData = selectActiveSurveyData(
      useProjectStore.getState(),
      useUIStore.getState().geometryMode
    ) ?? surveyDataFromLegacy(sections, stations);

    const issueSeverity = showIssueSeverity
      ? computeIssueSeverity(surveyData, warnings)
      : undefined;

    const features = buildSurveyFeatures(
      surveyData,
      loopClosures,
      this.gpsOrigin,
      this.showDepthGradient,
      this.depthRange,
      this.themeColors,
      this.cachedPrecision,
      { buildLabels, showIssueSeverity, issueSeverity },
      markers,
    );
    this.applyFeaturesToSources(features);
  }

  /**
   * Push a built `SurveyFeatureSets` to MapLibre sources and refresh the
   * derived state (hover/selection lookup maps, feature-state, etc.).
   * Shared by the synchronous `flushUpdate` path and the async
   * `flushUpdateViaWorker` path.
   */
  private applyFeaturesToSources(features: ReturnType<typeof buildSurveyFeatures>): void {
    if (!hasLiveStyle(this.map)) return;
    const {
      shotFeatures,
      shotGradientFeatures,
      shotLabelFeatures,
      stationFeatures,
      sectionLabelFeatures,
      stationLabelFeatures,
      loopClosureFeatures,
      markerFeatures,
    } = features;

    // Build section→featureId lookup maps for O(1) hover highlighting
    // (replaces per-hover querySourceFeatures which was the main perf bottleneck)
    this.sectionShotFeatureIds = new Map();
    for (const f of shotFeatures) {
      const sid = f.properties?.sectionId as number | undefined;
      const fid = f.properties?.featureId as number | undefined;
      if (sid !== undefined && fid !== undefined) {
        let set = this.sectionShotFeatureIds.get(sid);
        if (!set) { set = new Set(); this.sectionShotFeatureIds.set(sid, set); }
        set.add(fid);
      }
    }
    this.sectionStationFeatureIds = new Map();
    for (const f of stationFeatures) {
      const sid = f.properties?.sectionId as number | undefined;
      const fid = f.properties?.featureId as number | undefined;
      if (sid !== undefined && fid !== undefined) {
        let set = this.sectionStationFeatureIds.get(sid);
        if (!set) { set = new Set(); this.sectionStationFeatureIds.set(sid, set); }
        set.add(fid);
      }
    }

    // Update sources
    const shotsSource = this.map.getSource(this.sourceId + '-shots');
    if (shotsSource && 'setData' in shotsSource) {
      (shotsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: shotFeatures,
      });
      recordFeatureCount('survey-shots', shotFeatures.length);
    }

    const stationsSource = this.map.getSource(this.sourceId + '-stations');
    if (stationsSource && 'setData' in stationsSource) {
      (stationsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: stationFeatures,
      });
      recordFeatureCount('survey-stations', stationFeatures.length);
    }

    const shotGradientsSource = this.map.getSource(this.sourceId + '-shot-gradients');
    if (shotGradientsSource && 'setData' in shotGradientsSource) {
      (shotGradientsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: shotGradientFeatures,
      });
      recordFeatureCount('survey-shot-gradients', shotGradientFeatures.length);
    }

    // Update shot labels source
    const shotLabelsSource = this.map.getSource(this.sourceId + '-shot-labels');
    if (shotLabelsSource && 'setData' in shotLabelsSource) {
      (shotLabelsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: shotLabelFeatures,
      });
      recordFeatureCount('survey-shot-labels', shotLabelFeatures.length);
    }

    // Update section labels source
    const sectionLabelsSource = this.map.getSource(this.sourceId + '-section-labels');
    if (sectionLabelsSource && 'setData' in sectionLabelsSource) {
      (sectionLabelsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: sectionLabelFeatures,
      });
      recordFeatureCount('survey-section-labels', sectionLabelFeatures.length);
    }

    // Update station labels source
    const stationLabelsSource = this.map.getSource(this.sourceId + '-station-labels');
    if (stationLabelsSource && 'setData' in stationLabelsSource) {
      (stationLabelsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: stationLabelFeatures,
      });
      recordFeatureCount('survey-station-labels', stationLabelFeatures.length);
    }

    const loopClosuresSource = this.map.getSource(this.sourceId + '-loop-closures');
    if (loopClosuresSource && 'setData' in loopClosuresSource) {
      (loopClosuresSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: loopClosureFeatures,
      });
      recordFeatureCount('survey-loop-closures', loopClosureFeatures.length);
    }

    const markersSource = this.map.getSource(this.sourceId + '-markers');
    if (markersSource && 'setData' in markersSource) {
      (markersSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: markerFeatures,
      });
      recordFeatureCount('survey-markers', markerFeatures.length);
    }

    // setData() clears all feature-state — reset tracking and re-apply
    // Guard: only re-apply if map style is still live (avoids crash during teardown)
    if (!hasLiveStyle(this.map)) return;
    this.prevHoveredStationFeatureId = null;
    this.prevHoveredShotFeatureId = null;
    this.prevSelectedStationFeatureId = null;
    this.prevSelectedShotFeatureId = null;
    this.prevHoveredSectionShotIds = new Set();
    this.prevHoveredSectionStationIds = new Set();
    this.prevSelectedSectionShotIds = new Set();
    this.prevSelectedSectionStationIds = new Set();
    this.invalidateSectionCaches();
    this.updateVisualState();
    // setData() wiped feature-state, so re-apply the distance-gradient `dist`
    // HERE (the rebuild path) — NOT in updateVisualState(), which also runs on
    // every hover where feature-state is intact and re-applying all ~N features
    // would tank FPS (60→10 on large surveys). (#34)
    if (this.distanceMode) {
      this.applyDistanceFeatureState();
    }
    // Gas reachable-frontier overlay (#38) — same setData()-wipe re-apply.
    if (this.gasReachMode) {
      this.applyGasReachFeatureState();
    }
    // Likewise re-apply the measured along-line path tint (#26).
    if (this.measuredPathIds || this.measuredPathStationIds) {
      this.applyMeasuredPathFeatureState();
    }
    // Gas route highlight (#233): re-push the route LineStrings (coords come from cachedStations,
    // which a rebuild may have refreshed).
    if (this.gasRouteLegs.length > 0) {
      this.applyGasRoute();
    }

    // Fire one-shot callback (used by drag commit to restore filters in the same
    // synchronous block as setData, ensuring no paint gap between them).
    if (this.onNextUpdateCallback) {
      const cb = this.onNextUpdateCallback;
      this.onNextUpdateCallback = null;
      cb();
    }
  }

  /**
   * Register a one-shot callback to run at the end of the next update() call,
   * after all setData() calls have completed. Used to restore drag preview filters
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

    // Update paint properties that use constant theme colors
    // Section label halo
    if (this.map.getLayer(this.labelsLayerId)) {
      this.map.setPaintProperty(this.labelsLayerId, 'text-halo-color', colors.textHalo);
    }
    // Shot label halo
    if (this.map.getLayer(this.shotLabelsLayerId)) {
      this.map.setPaintProperty(this.shotLabelsLayerId, 'text-halo-color', colors.textHaloAlt);
    }
    // Station label halo
    if (this.map.getLayer(this.stationLabelsLayerId)) {
      this.map.setPaintProperty(this.stationLabelsLayerId, 'text-halo-color', colors.textHaloAlt);
    }
    // Comments label text-color (via applyCommentTextColor() to keep the
    // move-comment palette tint (#52) if that tool is currently active)
    this.applyCommentTextColor();
    // Station comments halo
    if (this.map.getLayer(this.stationCommentsLayerId)) {
      this.map.setPaintProperty(this.stationCommentsLayerId, 'text-halo-color', colors.textHaloAlt);
    }
    // Shot comments halo
    if (this.map.getLayer(this.shotCommentsLayerId)) {
      this.map.setPaintProperty(this.shotCommentsLayerId, 'text-halo-color', colors.textHaloAlt);
    }
    // Ghost station stroke
    if (this.map.getLayer(this.ghostStationsLayerId)) {
      this.map.setPaintProperty(this.ghostStationsLayerId, 'circle-stroke-color', colors.strokeColor);
    }
    // Ghost merge station
    if (this.map.getLayer(this.ghostMergeLayerId)) {
      this.map.setPaintProperty(this.ghostMergeLayerId, 'circle-color', colors.selectHighlight);
      this.map.setPaintProperty(this.ghostMergeLayerId, 'circle-stroke-color', colors.strokeColor);
    }
    // Marker circle stroke
    if (this.map.getLayer(this.markersLayerId)) {
      this.map.setPaintProperty(this.markersLayerId, 'circle-stroke-color', colors.strokeColor);
    }
    // Marker label halo
    if (this.map.getLayer(this.markerLabelsLayerId)) {
      this.map.setPaintProperty(this.markerLabelsLayerId, 'text-halo-color', colors.textHaloAlt);
    }
    // Station highlight (snap color)
    if (this.map.getLayer(this.stationsHighlightLayerId)) {
      this.map.setPaintProperty(this.stationsHighlightLayerId, 'circle-color', colors.snapColor);
      this.map.setPaintProperty(this.stationsHighlightLayerId, 'circle-stroke-color', colors.snapColor);
    }
    // Station stroke-color expression (hover/selected colors)
    if (this.map.getLayer(this.stationsLayerId)) {
      this.map.setPaintProperty(this.stationsLayerId, 'circle-stroke-color', [
        'case',
        ['boolean', ['feature-state', 'hover'], false], colors.hoverHighlight,
        ['boolean', ['feature-state', 'selected'], false], colors.selectHighlight,
        ['get', 'strokeColor'],
      ]);
    }

    // Rebuild features with new theme colors (colors are embedded in GeoJSON properties)
    if (this.cachedSections.length > 0 || this.cachedStations.length > 0) {
      this.update(
        this.cachedSections,
        this.cachedStations,
        this.cachedLoopClosures,
        this.gpsOrigin,
        this.showDepthGradient,
        this.depthRange,
        this.cachedPrecision,
        this.showIssueSeverity,
        this.cachedWarnings,
        this.cachedMarkers,
      );
    }
  }

  destroy() {
    // Cancel any pending coalesced update so its microtask doesn't fire on a
    // destroyed map (would throw in setData).
    this.updateScheduled = false;
    this.pendingUpdateArgs = null;

    // Zero memory-probe feature counts for our sources so the debug panel
    // stops showing stale counts from the last setData call.
    clearFeatureCountsForPrefix('survey-');

    // Cancel any pending hover RAF
    if (this.hoverRafId !== null) {
      cancelAnimationFrame(this.hoverRafId);
      this.hoverRafId = null;
    }
    this.pendingHoverEvent = null;

    // Clean up touch long-press timer
    if (this.touchLongPressTimer) {
      clearTimeout(this.touchLongPressTimer);
      this.touchLongPressTimer = null;
    }

    // Guard against map being destroyed (race condition on unmount)
    if (!hasLiveStyle(this.map)) {
      // Clear bound handler references even if map is destroyed
      this.boundMouseMove = null;
      this.boundMapLeave = null;
      this.boundTouchStart = null;
      this.boundTouchMove = null;
      this.boundTouchEnd = null;
      return;
    }

    try {
      // Unregister event handlers first (before removing layers)
      this.map.off('click', this.handleMapClick);
      this.map.off('contextmenu', this.handleContextMenu);

      if (this.boundMouseMove) {
        this.map.off('mousemove', this.boundMouseMove);
      }
      if (this.boundMapLeave) {
        this.map.getCanvas().removeEventListener('mouseleave', this.boundMapLeave);
      }
      // Remove touch long-press listeners
      const canvas = this.map.getCanvas();
      if (this.boundTouchStart) {
        canvas.removeEventListener('touchstart', this.boundTouchStart);
      }
      if (this.boundTouchMove) {
        canvas.removeEventListener('touchmove', this.boundTouchMove);
      }
      if (this.boundTouchEnd) {
        canvas.removeEventListener('touchend', this.boundTouchEnd);
        canvas.removeEventListener('touchcancel', this.boundTouchEnd);
      }
      this.map.off('zoomend', this.handleZoomEndTelemetry);
      this.map.off('moveend', this.handleMoveEndTelemetry);
      if (this.boundZoomEndForLabels) {
        this.map.off('zoomend', this.boundZoomEndForLabels);
        this.boundZoomEndForLabels = null;
      }

      // Remove all layers first (must be done before removing their sources)
      if (this.map.getLayer(this.labelsLayerId)) {
        this.map.removeLayer(this.labelsLayerId);
      }
      if (this.map.getLayer(this.shotLabelsLayerId)) {
        this.map.removeLayer(this.shotLabelsLayerId);
      }
      if (this.map.getLayer(this.stationLabelsLayerId)) {
        this.map.removeLayer(this.stationLabelsLayerId);
      }
      if (this.map.getLayer(this.stationsLayerId)) {
        this.map.removeLayer(this.stationsLayerId);
      }
      if (this.map.getLayer(this.stationsHighlightLayerId)) {
        this.map.removeLayer(this.stationsHighlightLayerId);
      }
      if (this.map.getLayer(this.shotsLayerId)) {
        this.map.removeLayer(this.shotsLayerId);
      }
      if (this.map.getLayer(this.shotGradientsLayerId)) {
        this.map.removeLayer(this.shotGradientsLayerId);
      }
      if (this.map.getLayer(this.measurePathLayerId)) {
        this.map.removeLayer(this.measurePathLayerId);
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
      // Remove hitbox layers
      if (this.map.getLayer(this.stationsHitboxLayerId)) {
        this.map.removeLayer(this.stationsHitboxLayerId);
      }
      if (this.map.getLayer(this.shotsHitboxLayerId)) {
        this.map.removeLayer(this.shotsHitboxLayerId);
      }
      if (this.map.getLayer(this.ghostStationsHitboxLayerId)) {
        this.map.removeLayer(this.ghostStationsHitboxLayerId);
      }
      if (this.map.getLayer(this.ghostShotsHitboxLayerId)) {
        this.map.removeLayer(this.ghostShotsHitboxLayerId);
      }

      // Remove all sources after layers are removed
      if (this.map.getSource(this.sourceId + '-shots')) {
        this.map.removeSource(this.sourceId + '-shots');
      }
      if (this.map.getSource(this.sourceId + '-shot-gradients')) {
        this.map.removeSource(this.sourceId + '-shot-gradients');
      }
      if (this.map.getSource(this.sourceId + '-stations')) {
        this.map.removeSource(this.sourceId + '-stations');
      }
      if (this.map.getSource(this.sourceId + '-loop-closures')) {
        this.map.removeSource(this.sourceId + '-loop-closures');
      }
      if (this.map.getSource(this.sourceId + '-section-labels')) {
        this.map.removeSource(this.sourceId + '-section-labels');
      }
      if (this.map.getSource(this.sourceId + '-shot-labels')) {
        this.map.removeSource(this.sourceId + '-shot-labels');
      }
      if (this.map.getSource(this.sourceId + '-station-labels')) {
        this.map.removeSource(this.sourceId + '-station-labels');
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
    } catch {
      // Map already destroyed during unmount, cleanup not needed
    }

    // Clear bound handler references
    this.boundMouseMove = null;
    this.boundMapLeave = null;
  }
}
