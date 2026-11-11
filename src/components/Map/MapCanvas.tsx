import { useEffect, useLayoutEffect, useRef, useCallback, useState, useMemo, memo, startTransition } from 'react';
import { useTranslation } from 'react-i18next';
import * as maplibregl from 'maplibre-gl';
import type { SectionId, ShotId, StationRef, DistanceField } from '../../types/project';
import type { GasReachField } from '../../types/ipc.generated';
import type { BatchCommand } from '../../types/commands';
import { useProjectStore } from '../../store/projectStore';
import {
  useUIStore,
  isMapReadOnly,
  showsClosureAdjustedNotice,
} from '../../store/uiStore';
import { pickUIKeys, type UIStoreState } from '../../store/uiSelectKeys';
import { selectComputeGate, useGasPlanStore } from '../../store/gasPlanStore';
import { selectActiveStations, selectActiveSurveyData } from '../../store/geometrySelectors';
import { isMutatingTool, shouldResetNavigationTool, toolCursor } from '../../utils/toolUtils';
import { mapGestureMode } from '../../utils/mapGestures';
import { attachDragReleaseForwarder } from '../../utils/mapDragRelease';
import { wheelZoomRateForLevels, ZOOM_LEVELS_PER_NOTCH_DEFAULT } from '../../utils/zoomRate';
import { hasEligibleClosures } from '../../utils/closures';
import { useShallow } from 'zustand/react/shallow';
import {
  getEffectiveShowJumps,
  getEffectiveShowReferenceShots,
  getEffectiveShowShotData,
  getEffectiveShowStationComments,
  getEffectiveShowShotComments,
  getEffectiveStationLabelMode,
  isShotKindVisible,
} from '../../store/displaySelectors';
import { getErrorMessage, invokeIpc } from '../../utils/ipc';
import { buildDistanceFeatureState, overlayApplyAction } from '../../utils/distanceGradient';
import { buildGasReachFeatureState } from '../../utils/gasReach';
import { segmentColor, routeLegSegmentIndices } from '../../utils/segmentColors';
import { buildMeasuredPathIds, buildMeasuredStationIds, MEASURE_TINT } from '../../utils/measurePath';
import { computeImmovableStationKeys } from '../../utils/immovableStations';
import { installMapE2eHooks } from '../../dev/mapE2eHooks';
import { getBounds, getBoundsFromStationCoords, metersToMapCoords, mapCoordsToMeters } from '../../utils/coordinates';
import { registerTileProtocol, unregisterTileProtocol } from '../../utils/tileProtocol';
import { getTileConfig } from '../../utils/tileConfig';
import { useMapTools } from '../../hooks/useMapTools';
import { useGasPlanPointMarkers } from './useGasPlanPointMarkers';
import { useGasPlanDropMarkers } from './useGasPlanDropMarkers';
import SurveyLayer, { type SurveyLayerCallbacks } from './SurveyLayer';
import { SATELLITE_BOTTOM_CANDIDATES } from './mapUtils';
import { renderFatalMessage } from './surveyLayer/mapErrors';
import PlacementOverlay from './PlacementOverlay';
import GridOverlay from './GridOverlay';
import { GasLegend } from './GasLegend';
import { MapDevStatsPanels } from './MapDevStatsPanels';
import CommentOverlay from './CommentOverlay';
import MoveCommentCursorTag from './MoveCommentCursorTag';
import { installPanZoomDebug } from './panZoomDebug';
import { createMap } from './createMap';
import { MapUnavailable } from './MapUnavailable';
import { getMapThemeColors } from '../../utils/themeColors';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { MapTopLeftOverlay } from './MapTopLeftOverlay';
import { useIssueRenderInput } from './useIssueDisplay';
import { useSpeleoStore, selectSpeleoReadOnly } from '../../store/speleoStore';
import { useNetworkStore } from '../../store/networkStore';
import { FEATURE_ID_STRIDE } from '../../utils/ids';
import { debugLog } from '../../utils/debugLog';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useSurveyLayerColorScales } from './colorScale/useColorScaleInputs';
import { MapColorLegends } from './MapColorLegends';

/**
 * How far a WATCHED Ctrl+left press has to travel before the custom pan takes
 * it over. Equal to MapLibre's default `clickTolerance` (map init does not
 * override it), which is the distance at which MapLibre itself stops calling a
 * press-and-release a click: below it the press is left alone so Ctrl+click can
 * toggle a feature's selection, above it MapLibre suppresses its own click and
 * the gesture is unambiguously a drag.
 */
const CUSTOM_PAN_CLAIM_PX = 3;

interface ZoomBoxState {
  start: { x: number; y: number } | null;
  end: { x: number; y: number } | null;
}

interface MoveDragState {
  type: 'station' | 'segment' | 'shot';
  sectionId?: number;
  stationRef?: { section_id: number; station_index: number };
  shotIndex?: number;
  connectedSectionIds?: number[];
  startLngLat: { lng: number; lat: number };
  currentLngLat: { lng: number; lat: number };
}

// Helper to convert StationRef to highlight ID format (section_id:station_index)
// Moved outside component to avoid recreating on each render
const stationRefToId = (ref: { section_id: number; station_index: number } | null): string | null => {
  if (!ref) return null;
  return `${ref.section_id}:${ref.station_index}`;
};

// Screen pixels per meter at the map center, by projecting a 1 meter step north
// (1 meter = 1/111320 degrees). Feeds the to-scale placement preview only, so it
// is computed on demand rather than on every zoom frame (#430).
const pixelsPerMeterAt = (map: maplibregl.Map): number => {
  const center = map.getCenter();
  const centerScreen = map.project(center);
  const northPoint = map.project([center.lng, center.lat + 1 / 111320]);
  const ppm = Math.abs(centerScreen.y - northPoint.y);
  return ppm > 0 ? ppm : 1;
};

/**
 * Lightweight child component that subscribes to hover state and syncs it to
 * SurveyLayer. Extracted from MapCanvas so that hover changes (~60/sec during
 * mouse movement) only re-render this tiny component instead of the entire
 * 2700-line MapCanvas with ~50 hooks.
 */
function MapHoverSync({
  surveyLayerRef,
  mapLoaded,
  anchoredSectionIds,
  getConnectedSectionIds,
  logMapInteractions,
}: {
  surveyLayerRef: React.RefObject<SurveyLayer | null>;
  mapLoaded: boolean;
  anchoredSectionIds: Set<number>;
  getConnectedSectionIds: (id: number) => Promise<number[]>;
  logMapInteractions: boolean;
}) {
  const hoveredStation = useUIStore((s) => s.hoveredStation);
  const hoveredShot = useUIStore((s) => s.hoveredShot);
  const hoveredSectionId = useUIStore((s) => s.hoveredSectionId);
  const activeTool = useUIStore((s) => s.activeTool);

  // Sync hoveredSectionId to SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setHoveredSection(hoveredSectionId);
  }, [hoveredSectionId, mapLoaded, surveyLayerRef]);

  // Sync hoveredShot to SurveyLayer (for DataTable row hover)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setHoveredShot(
      hoveredShot?.sectionId ?? null,
      hoveredShot?.shotId ?? null
    );
  }, [hoveredShot, mapLoaded, surveyLayerRef]);

  // Sync hoveredStation to SurveyLayer (for DetailLink hover)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setHoveredStation(hoveredStation);
  }, [hoveredStation, mapLoaded, surveyLayerRef]);

  // Resolve connected segment on hover when Move Segment tool is active.
  const segmentHoverTokenRef = useRef(0);
  useEffect(() => {
    if (!surveyLayerRef.current) return undefined;

    if (activeTool !== 'move_segment') {
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover cleared (tool inactive)', {
          activeTool,
        });
      }
      surveyLayerRef.current.setHoveredSegment([]);
      return undefined;
    }

    const sectionId = hoveredStation?.section_id ?? hoveredShot?.sectionId ?? null;

    if (sectionId === null) {
      segmentHoverTokenRef.current++;
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover pending clear (no hovered feature)', {});
      }
      const timer = setTimeout(() => {
        surveyLayerRef.current?.setHoveredSegment([]);
      }, 150);
      return () => clearTimeout(timer);
    }

    if (anchoredSectionIds.has(sectionId)) {
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover suppressed (anchored section)', {
          sectionId,
        });
      }
      surveyLayerRef.current.setHoveredSegment([]);
      return undefined;
    }

    const token = ++segmentHoverTokenRef.current;
    getConnectedSectionIds(sectionId).then((ids) => {
      if (segmentHoverTokenRef.current !== token) return;
      const movable = ids.filter((id) => !anchoredSectionIds.has(id));
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover resolved', {
          sectionId,
          connectedCount: ids.length,
          movableCount: movable.length,
        });
      }
      surveyLayerRef.current?.setHoveredSegment(movable);
    }).catch(() => {
      if (segmentHoverTokenRef.current !== token) return;
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover fallback (IPC failed)', {
          sectionId,
        });
      }
      surveyLayerRef.current?.setHoveredSegment([sectionId]);
    });
    return undefined;
  }, [activeTool, hoveredStation, hoveredShot, anchoredSectionIds, getConnectedSectionIds, mapLoaded, logMapInteractions, surveyLayerRef]);

  return null;
}

/**
 * The uiStore keys MapCanvas reads (#430). Explicit, so the subscription costs a
 * 63-key shallow compare instead of a ~180-key one and can actually bail out
 * while the camera moves. The hover keys (hoveredStation / hoveredShot /
 * hoveredSectionId) are deliberately ABSENT: they change ~60x/s during mouse
 * movement and would re-render this 4000-line component every frame - MapHoverSync
 * subscribes to them instead. measureSnapShot is absent for the same reason (the
 * live shot-snap `t` is rewritten on every mousemove over a shot and the cursor dot
 * is driven imperatively via refs); the leave-tool cleanup reads it via getState().
 */
const MAP_CANVAS_UI_KEYS = [
  // NOTE: `mapZoom` is deliberately ABSENT. Its only read is the initial-camera
  // restore inside the empty-dep map-creation effect, which can never observe a
  // later store write anyway, so subscribing would only re-render this whole
  // component on every 'zoomend'. That site reads it via getState() instead.
  'setMapZoom',
  'activeTool',
  'showSatellite',
  'showShotData',
  'showSectionNames',
  'showStationLabels',
  'showStationNames',
  'showStationComments',
  'showShotComments',
  'showStations',
  'showShots',
  'showReferenceShots',
  'showJumps',
  'showDepthGradient',
  'showDistanceGradient',
  'appMode',
  'showIssueSeverity',
  'distanceGradientOrigin',
  'distanceField',
  'setDistanceField',
  'setDistanceGradientOrigin',
  'showDevStats',
  'showMapStats',
  'showZoomStats',
  'selection',
  'mergeStationA',
  'mergeStationB',
  'measureStart',
  'measureEnd',
  'measureDirect',
  'measureAlongLine',
  'measureAlongLinePending',
  'setMeasureSnapShot',
  'clearMeasure',
  'clearSelection',
  'clearSelectionByType',
  'filterSelection',
  'clearMergeStations',
  'requestFitToBounds',
  'clearFitToBoundsRequest',
  'snappedStation',
  'setSnappedStation',
  'setHoveredStation',
  'setHoveredShot',
  'placementMode',
  'updatePlacementCursor',
  'updatePlacementSnap',
  'cancelPlacementMode',
  'confirmPlacement',
  'showToast',
  'requestZoomToSection',
  'clearZoomToSectionRequest',
  'requestPanToShot',
  'clearPanToShotRequest',
  'requestCenterOnStation',
  'clearCenterOnStationRequest',
  'requestCenterOnShot',
  'clearCenterOnShotRequest',
  'requestZoomToShot',
  'clearZoomToShotRequest',
  'geometryPreview',
  'resolvedTheme',
  'moveCommentSource',
  'detailPanelOpen',
  'detailPanelTab',
] as const satisfies readonly (keyof UIStoreState)[];

const selectMapCanvasUI = (s: UIStoreState) => pickUIKeys(s, MAP_CANVAS_UI_KEYS);

function MapCanvas() {
  const precision = useDecimalPrecision();
  const mapContainer = useRef<HTMLDivElement>(null);
  const map = useRef<maplibregl.Map | null>(null);
  const surveyLayerRef = useRef<SurveyLayer | null>(null);
  const mapInteractionSequenceRef = useRef(0);
  const originMarkerRef = useRef<maplibregl.Marker | null>(null);
  const gasOriginMarkerRef = useRef<maplibregl.Marker | null>(null);
  const scaleControlRef = useRef<maplibregl.ScaleControl | null>(null);
  const scaleControlAddedRef = useRef<boolean>(false);
  const [zoomBox, setZoomBox] = useState<ZoomBoxState>({ start: null, end: null });
  const [moveDrag, setMoveDrag] = useState<MoveDragState | null>(null);
  const [mapLoaded, setMapLoaded] = useState(false);
  const [webgl2Unavailable, setWebgl2Unavailable] = useState(false);
  const [ctrlHeld, setCtrlHeld] = useState(false);
  // True briefly after placement confirmation to suppress the spurious MapLibre click
  const placementJustConfirmedRef = useRef(false);
  // True while custom panning gesture is active (Ctrl+left-drag or middle-drag)
  const customPanActiveRef = useRef(false);
  // True while custom panning was initiated with Ctrl+left-drag (for keyup filtering)
  const ctrlPanActiveRef = useRef(false);
  // Timestamp of the last mouse event during a Ctrl+left-drag (for spurious keyup filtering)
  const ctrlPanMouseEventTimeRef = useRef(0);
  const [isHoveringFeature, setIsHoveringFeature] = useState(false);
  // Placement mode cursor position (screen pixels)
  const [placementCursor, setPlacementCursor] = useState<{ x: number; y: number } | null>(null);
  // Current map scale in pixels per meter (for to-scale placement preview)
  const [pixelsPerMeter, setPixelsPerMeter] = useState(1);
  // RAF batching for snap-to-station checks (prevents event queue buildup on mousemove)
  const snapRafId = useRef<number | null>(null);
  const pendingSnapPosition = useRef<{ x: number; y: number } | null>(null);
  // Ref mirror of snappedStation - used for same-value comparison in the snap hot path
  // so checkSnapToStation doesn't depend on React state (avoids re-render in the loop)
  const snappedStationRef = useRef<StationRef | null>(null);
  // Ref mirror of the measure shot-snap candidate (#26) + its live cursor marker,
  // updated imperatively in the snap hot path like the station snap.
  const measureSnapShotRef = useRef<{ section_id: number; shot_id: number; t: number; lng: number; lat: number } | null>(null);
  const shotSnapMarkerRef = useRef<maplibregl.Marker | null>(null);
  // Pending section zoom request (stored when map not loaded, processed when it loads)
  const pendingZoomToSectionRef = useRef<number | null>(null);
  // Pending pan-to-shot request (stored when map not loaded, processed when it loads)
  const pendingPanToShotRef = useRef<{ sectionId: number; shotId: number } | null>(null);
  // Pending center-on-station request (stored when map not loaded, processed when it loads)
  const pendingCenterOnStationRef = useRef<StationRef | null>(null);
  // Pending center-on-shot request (stored when map not loaded, processed when it loads)
  const pendingCenterOnShotRef = useRef<{ sectionId: number; shotId: number } | null>(null);
  // Anchored section IDs (cached when Move Segment tool is active)
  const [anchoredSectionIds, setAnchoredSectionIds] = useState<Set<number>>(new Set());
  // Whether anchored section IDs have been loaded for the current move_segment activation
  // Blocks segment drag until the set is populated to prevent moving anchored segments
  const [anchoredIdsLoaded, setAnchoredIdsLoaded] = useState(false);
  // Clear-on-failure flags for the analytical overlays (#34 distance / #38 gas):
  // set when a field recompute REJECTS so the apply effect drops the stale ramp
  // (base colors) instead of `overlayApplyAction` holding it against the new
  // inputs indefinitely. Reset on the next successful field (see the fetch effects).
  const [distanceFieldFailed, setDistanceFieldFailed] = useState(false);
  const [gasFieldFailed, setGasFieldFailed] = useState(false);
  // Token to invalidate stale getConnectedSectionIds results (incremented on each drag start,
  // checked in .then()/.catch() to discard late async results after mouseup)
  const segmentDragTokenRef = useRef(0);
  // --- Drag hot-path refs (bypass React state to avoid re-render per mousemove) ---
  // Current drag position (updated imperatively on every mousemove)
  const moveDragLngLatRef = useRef<{ lng: number; lat: number } | null>(null);
  // Last cursor position (client coords) during a move-drag - lets us re-apply the
  // preview on map zoom (when no pointer event fires) so it tracks across zoom (#79).
  const lastCursorClientRef = useRef<{ clientX: number; clientY: number } | null>(null);
  // Connected section IDs being dragged (used by rAF callback for segment preview)
  const dragConnectedSectionIdsRef = useRef<Set<number>>(new Set());
  // Origin station that initiated the segment drag (for snap-to-merge)
  const dragOriginStationRef = useRef<StationRef | null>(null);
  // Current snap target during drag (ref, not state - updated per mousemove)
  const dragSnapRef = useRef<StationRef | null>(null);
  // Last raw (pre-snap) pixel offset - used to restore preview if snap-merge is cancelled
  const lastRawDragOffsetRef = useRef<{ dx: number; dy: number } | null>(null);
  // True while snap-to-merge dialog is open - blocks mouseup from ending drag
  const snapMergeDialogPendingRef = useRef(false);
  // Co-located station refs for station drag (includes merged/closured stations)
  const stationDragColocatedRefsRef = useRef<StationRef[]>([]);
  // Where the live press landed (client coords), or null once that press has
  // travelled far enough to be a drag rather than a click. This is what
  // click-gates the move ghost's drop (#409): a plain drag pans the map, so only
  // a press that stays put may commit. Set on every pointerdown that reaches the
  // canvas, cleared by the release that ends it - and by the first pointermove
  // past the threshold, which latches the decision exactly as the custom pan's
  // own claim does (coming back to the press point cannot revive the drop).
  const pressClientRef = useRef<{ x: number; y: number } | null>(null);
  const { t } = useTranslation();

  const { project, stations, surveyData, stationsAdjusted, surveyDataAdjusted, adjustedLoading, loadAdjustedGeometry, config, moveStation, moveSegment, getConnectedSectionIds, getAnchoredSectionIds, executeBatch } = useProjectStore();
  const logMapInteractions = config?.debug?.map?.log_hit_detection ?? false;

  // Geometry display mode (measured vs read-only closure-adjusted).
  const geometryMode = useUIStore((s) => s.geometryMode);
  // Derived once for the top-left overlay, so its two gates share a single raw
  // comparison and `scripts/check-readonly-gates.mjs`'s exact-count allow-list for
  // this file stays put. NOT an editability gate - that is `useEditingBlocked()`.
  const closureAdjustedView = geometryMode === 'closureAdjusted';
  // T2b: a remote SpeleoDB project opened without the write lock shows a
  // top-left read-only badge, mirroring the closure-adjusted view-only badge.
  const speleoReadOnly = useSpeleoStore(selectSpeleoReadOnly);
  const setGeometryMode = useUIStore((s) => s.setGeometryMode);
  const setActiveTool = useUIStore((s) => s.setActiveTool);

  // #50: section-focus mode - show an exit-focus control at the map's top-left
  // while focused.
  const focusedSectionId = useUIStore((s) => s.focusedSectionId);
  const exitSectionFocus = useUIStore((s) => s.exitSectionFocus);

  // Station coordinates for the active display mode. The map renders the active
  // geometry; coordinate-derived editing tools are disabled in the adjusted
  // (read-only) view, so other consumers continue to use measured `stations`.
  const activeStations = useMemo(
    () => selectActiveStations({ stations, surveyData, stationsAdjusted, surveyDataAdjusted }, geometryMode),
    [stations, surveyData, stationsAdjusted, surveyDataAdjusted, geometryMode]
  );
  // Active struct-of-arrays survey data, for derived values (e.g. depth range)
  // that must scale to the geometry actually drawn.
  const activeSurveyData = useMemo(
    () => selectActiveSurveyData({ stations, surveyData, stationsAdjusted, surveyDataAdjusted }, geometryMode),
    [stations, surveyData, stationsAdjusted, surveyDataAdjusted, geometryMode]
  );

  // Get GPS origin for coordinate transforms (enables satellite tile alignment)
  // Priority: project's zero-datum GPS > config's satellite_gps_origin
  const gpsOrigin = useMemo(() => {
    const zeroDatum = project?.zero_datums?.[0];
    if (zeroDatum?.gps) {
      return { latitude: zeroDatum.gps.latitude, longitude: zeroDatum.gps.longitude };
    }
    return config?.map.satellite_gps_origin ?? null;
  }, [project?.zero_datums, config?.map.satellite_gps_origin]);

  // Compute depth range from both station coordinates and shot depth endpoints.
  // Including shot depths avoids a flat scale when station Z isn't populated yet.
  const depthRange = useMemo(() => {
    // SoA iteration: walks typed arrays instead of triggering the
    // lazy `section.shots` getter (which would materialize Shot
    // objects for every section). For 75k shots + 89k stations
    // this drops from "force-materialize all" to a 3 × O(N) typed
    // array pass. When `surveyData` isn't available (very early in
    // initial load or unusual init paths), fall back to the legacy
    // shape - accepted regression cost in that corner case.
    let min = Infinity;
    let max = -Infinity;
    if (activeSurveyData) {
      // Station z coordinates live at stride 3 in stationCoords:
      // [x0,y0,z0, x1,y1,z1, …]. Read directly from the typed array.
      const coords = activeSurveyData.stationCoords;
      for (let i = 2; i < coords.length; i += 3) {
        const z = coords[i]!;
        if (z < min) min = z;
        if (z > max) max = z;
      }
      const depthIn = activeSurveyData.shotDepthIn;
      const depthOut = activeSurveyData.shotDepthOut;
      for (let i = 0; i < activeSurveyData.shotCount; i++) {
        const di = depthIn[i]!;
        const dout = depthOut[i]!;
        if (di < min) min = di;
        if (di > max) max = di;
        if (dout < min) min = dout;
        if (dout > max) max = dout;
      }
    } else {
      if (activeStations && activeStations.length > 0) {
        for (const station of activeStations) {
          const z = station.coordinates.z;
          if (z < min) min = z;
          if (z > max) max = z;
        }
      }
      const sections = project?.sections ?? [];
      for (const section of sections) {
        for (const shot of section.shots) {
          if (shot.depth_in < min) min = shot.depth_in;
          if (shot.depth_in > max) max = shot.depth_in;
          if (shot.depth_out < min) min = shot.depth_out;
          if (shot.depth_out > max) max = shot.depth_out;
        }
      }
    }
    if (!Number.isFinite(min) || !Number.isFinite(max)) return { min: 0, max: 50 };
    return { min, max };
  }, [activeSurveyData, activeStations, project?.sections]);

  // Extract zoom config values for stable dependencies (used by zoom handlers and effects)
  const zoomStep = config?.map.zoom_step ?? 1.0;
  const zoomMin = config?.map.zoom_min ?? 0;
  const zoomMax = config?.map.zoom_max ?? 22;

  // Performance: subscribe to an EXPLICIT key list (MAP_CANVAS_UI_KEYS above), which
  // excludes the hover values (hoveredStation, hoveredShot, hoveredSectionId) and
  // measureSnapShot. Those change ~60 times/sec during mouse movement and would cause
  // this 4000-line component to re-render every frame. Hover sync is handled by the
  // <MapHoverSync> child component instead; measureSnapShot is read via getState().
  const {
    setMapZoom,
    activeTool,
    showSatellite,
    showShotData,
    showSectionNames,
    showStationLabels,
    showStationNames,
    showStationComments,
    showShotComments,
    showStations,
    showShots,
    showReferenceShots,
    showJumps,
    showDepthGradient,
    showDistanceGradient,
    appMode,
    showIssueSeverity,
    distanceGradientOrigin,
    distanceField,
    setDistanceField,
    setDistanceGradientOrigin,
    showDevStats,
    showMapStats,
    showZoomStats,
    selection,
    mergeStationA,
    mergeStationB,
    measureStart,
    measureEnd,
    measureDirect,
    measureAlongLine,
    measureAlongLinePending,
    setMeasureSnapShot,
    clearMeasure,
    clearSelection,
    clearSelectionByType,
    filterSelection,
    clearMergeStations,
    requestFitToBounds,
    clearFitToBoundsRequest,
    snappedStation,
    setSnappedStation,
    setHoveredStation,
    setHoveredShot,
    placementMode,
    updatePlacementCursor,
    updatePlacementSnap,
    cancelPlacementMode,
    confirmPlacement,
    showToast,
    requestZoomToSection,
    clearZoomToSectionRequest,
    requestPanToShot,
    clearPanToShotRequest,
    requestCenterOnStation,
    clearCenterOnStationRequest,
    requestCenterOnShot,
    clearCenterOnShotRequest,
    requestZoomToShot,
    clearZoomToShotRequest,
    geometryPreview,
    resolvedTheme,
    moveCommentSource,
    detailPanelOpen,
    detailPanelTab,
  } = useUIStore(useShallow(selectMapCanvasUI));

  // The gas overlay belongs to the Plan workspace mode (the former showGasPlan
  // sub-mode). Derived so the existing showGasPlan-gated effects below are unchanged.
  const showGasPlan = appMode === 'plan';

  // Gas & route-planning sub-mode inputs (#38) - a separate store so editing the
  // diver/cylinders/config doesn't re-render via the big uiStore subscription.
  const {
    originChosen: gasOriginChosen,
    origin: gasOrigin,
    waypoints: gasWaypoints,
    segments: gasSegments,
    markedPoints: gasMarkedPoints,
    diver: gasDiver,
    cylinders: gasCylinders,
    gasConfig: gasPlanConfig,
    reachField: gasReachField,
    setReachField: setGasReachField,
    setOrigin: setGasOrigin,
    setStageDropOverride: setGasStageDropOverride,
  } = useGasPlanStore(
    useShallow((s) => ({
      originChosen: s.originChosen,
      origin: s.origin,
      waypoints: s.waypoints,
      segments: s.segments,
      markedPoints: s.markedPoints,
      diver: s.diver,
      cylinders: s.cylinders,
      gasConfig: s.gasConfig,
      reachField: s.reachField,
      setReachField: s.setReachField,
      setOrigin: s.setOrigin,
      setStageDropOverride: s.setStageDropOverride,
    }))
  );
  // #310: what the pins are allowed to send. A pin the backend could no longer place is withheld
  // here rather than sent stale - a stale positional ref names a REAL station, just the wrong one.
  const gasComputeGate = useGasPlanStore(useShallow(selectComputeGate));

  // Issue-severity coloring is active when the user enabled it OR while the
  // Warnings tab is open (auto-highlight, gated by the setting). It is DERIVED,
  // never written to the store: leaving the tab simply recomputes the colors from
  // the persisted toggles, so no "prior coloring" snapshot can be lost and every
  // toggle of a coloring mode just triggers a recompute (issue #57, finding #6).
  const highlightWarningsOnTabSwitch = config?.map.highlight_warnings_on_tab_switch ?? true;
  // The gas sub-mode (#38) swaps the detail panel away from Warnings but leaves
  // detailPanelTab='warnings' in the store, so the warnings-tab highlight would
  // otherwise stay derived-true and force needless severity rebuilds under the gas
  // overlay. Suppress it while gas owns the layer paint.
  const effectiveIssueSeverity =
    showIssueSeverity ||
    (detailPanelOpen && detailPanelTab === 'warnings' && highlightWarningsOnTabSwitch && !showGasPlan);
  // Severity takes visual precedence over the depth gradient when both apply.
  const effectiveDepthGradient = showDepthGradient && !effectiveIssueSeverity;
  // Null while Survey Issues is off, so warning-only refreshes don't rebuild.
  const issueInput = useIssueRenderInput(effectiveIssueSeverity, false);

  const alignedClosureIds = useMemo(() => {
    if (!project?.loop_closures) return new Set<bigint>();
    return new Set(
      project.loop_closures
        .filter((closure) => closure.is_aligned)
        .map((closure) => closure.id)
    );
  }, [project?.loop_closures]);

  // Compute immovable station keys for move_station and move_shot tools.
  // A station is immovable if it is zero-datum, or transitively reachable from a
  // zero-datum station via merged_with and aligned closures_with links. Reads the
  // SoA directly (computeImmovableStationKeys) rather than the lazy Station[]
  // proxy, which would rebuild + RETAIN the whole ~89k-object graph here (#mem).
  const immovableStationKeys = useMemo(() => {
    if (activeTool !== 'move_station' && activeTool !== 'move_shot') return new Set<string>();
    if (!surveyData) return new Set<string>();
    return computeImmovableStationKeys(surveyData, alignedClosureIds);
  }, [activeTool, surveyData, alignedClosureIds]);

  useEffect(() => {
    if (!logMapInteractions) return;
    console.debug('[Selection] Computed immovable stations', {
      count: immovableStationKeys.size,
      activeTool,
    });
  }, [logMapInteractions, immovableStationKeys, activeTool]);

  // Convert immovable station keys to feature IDs and compute immovable shot IDs.
  // For move_shot: a shot is immovable if either endpoint is immovable.
  const immovableFeatureIds = useMemo(() => {
    if (immovableStationKeys.size === 0) {
      return { stationIds: new Set<number>(), shotIds: new Set<number>() };
    }
    const stationIds = new Set<number>();
    for (const key of immovableStationKeys) {
      const [sec, idx] = key.split(':');
      stationIds.add(Number(sec) * FEATURE_ID_STRIDE + Number(idx));
    }
    const shotIds = new Set<number>();
    if (activeTool === 'move_shot') {
      for (const section of project?.sections ?? []) {
        for (let i = 0; i < section.shots.length; i++) {
          const fromKey = `${section.id}:${i}`;
          const toKey = `${section.id}:${i + 1}`;
          if (immovableStationKeys.has(fromKey) || immovableStationKeys.has(toKey)) {
            shotIds.add(section.id * FEATURE_ID_STRIDE + section.shots[i]!.id);
          }
        }
      }
    }
    return { stationIds, shotIds };
  }, [immovableStationKeys, activeTool, project?.sections]);

  // Clear selection from both the Zustand store and the SurveyLayer map visuals.
  // The direct SurveyLayer calls are needed because the useEffect that syncs
  // store → SurveyLayer runs after the next React render, leaving stale highlights
  // visible during operations that start immediately (e.g. drag preview).
  const clearSelectionAndVisuals = useCallback(() => {
    clearSelection();
    if (surveyLayerRef.current) {
      surveyLayerRef.current.setSelectedStation(null);
      surveyLayerRef.current.setSelectedShot(null, null);
      surveyLayerRef.current.setSelectedSection(null);
    }
  }, [clearSelection]);

  // Snap-to-station detection
  // Note: Snap detection works even when stations are hidden - the highlight layer
  // will show the snapped station. Merge tool is an exception (blocked when hidden).
  // Visual update goes directly to SurveyLayer (bypassing React re-render) so the
  // indicator tracks the cursor at frame rate - same strategy as hover detection.
  const checkSnapToStation = useCallback(
    (screenX: number, screenY: number) => {
      const clearStationSnap = () => {
        if (snappedStationRef.current) {
          snappedStationRef.current = null;
          surveyLayerRef.current?.setSnappedStation(null);
          startTransition(() => setSnappedStation(null));
        }
      };
      const clearShotSnap = () => {
        if (shotSnapMarkerRef.current) {
          shotSnapMarkerRef.current.remove();
          shotSnapMarkerRef.current = null;
        }
        if (measureSnapShotRef.current) {
          measureSnapShotRef.current = null;
          startTransition(() => setMeasureSnapShot(null));
        }
      };

      if (!map.current || !config || !surveyLayerRef.current) {
        clearStationSnap();
        clearShotSnap();
        return;
      }

      // Check if snap is enabled for the current tool
      // Note: Merge requires stations to be visible (can't select merge targets when hidden)
      const snapConfig = config.map.snap;
      const shouldSnap =
        (activeTool === 'merge' && snapConfig.enabled_merge && showStations) ||
        (activeTool === 'measure' && snapConfig.enabled_measure);

      if (!shouldSnap) {
        clearStationSnap();
        clearShotSnap();
        return;
      }

      const nearestStation = surveyLayerRef.current.queryNearestStation(
        screenX, screenY, snapConfig.threshold
      );
      // Measure (#26) also snaps to an exact point on a shot when no station is
      // within range - stations win when both are close.
      const nearestShot =
        activeTool === 'measure' && !nearestStation
          ? surveyLayerRef.current.queryNearestShotPoint(screenX, screenY, snapConfig.threshold)
          : null;

      // Station snap - compare via ref (no React state dependency in the hot path).
      const prev = snappedStationRef.current;
      if (nearestStation) {
        if (
          !prev ||
          prev.section_id !== nearestStation.section_id ||
          prev.station_index !== nearestStation.station_index
        ) {
          snappedStationRef.current = nearestStation;
          // Direct visual update - no React re-render needed
          surveyLayerRef.current.setSnappedStation(nearestStation);
          // Deferred store update for click handling - startTransition prevents
          // synchronous React re-render from blocking the current frame's paint
          startTransition(() => setSnappedStation(nearestStation));
        }
      } else {
        clearStationSnap();
      }

      // Shot-point snap (measure only): a live cursor marker (imperative, like the
      // station highlight) + a deferred store push for the click handler.
      if (nearestShot) {
        if (!shotSnapMarkerRef.current && map.current) {
          const el = document.createElement('div');
          el.setAttribute('data-testid', 'measure-shot-snap-marker');
          el.style.cssText =
            'width:11px;height:11px;border-radius:50%;background:#22d3ee;' +
            'border:2px solid #ffffff;box-shadow:0 0 3px rgba(0,0,0,0.6);pointer-events:none;';
          shotSnapMarkerRef.current = new maplibregl.Marker({ element: el })
            .setLngLat([nearestShot.lng, nearestShot.lat])
            .addTo(map.current);
        } else {
          shotSnapMarkerRef.current?.setLngLat([nearestShot.lng, nearestShot.lat]);
        }
        const prevShot = measureSnapShotRef.current;
        if (
          !prevShot ||
          prevShot.section_id !== nearestShot.section_id ||
          prevShot.shot_id !== nearestShot.shot_id ||
          prevShot.t !== nearestShot.t
        ) {
          measureSnapShotRef.current = nearestShot;
          startTransition(() =>
            setMeasureSnapShot({
              section_id: nearestShot.section_id,
              shot_id: nearestShot.shot_id,
              t: nearestShot.t,
            })
          );
        }
      } else {
        clearShotSnap();
      }
    },
    [activeTool, config, setSnappedStation, setMeasureSnapShot, showStations]
  );

  // Check snap for placement mode
  const checkPlacementSnap = useCallback(
    (screenX: number, screenY: number) => {
      if (!map.current || !config || !placementMode || !surveyLayerRef.current) {
        return null;
      }

      return surveyLayerRef.current.queryNearestStation(
        screenX, screenY, config.map.snap.threshold
      );
    },
    [config, placementMode]
  );

  // Handle placement confirmation
  const handlePlacementConfirm = useCallback(() => {
    if (!map.current || !placementMode || !mapContainer.current) return;

    const placement = confirmPlacement();
    if (!placement || !placement.cursorPosition) return;

    // Suppress the spurious MapLibre click that follows pointerDown on touch devices.
    // The browser synthesizes click after pointerup, but placementMode is already null
    // by then, so the click would be treated as an empty-click and clear selection.
    placementJustConfirmedRef.current = true;
    requestAnimationFrame(() => { placementJustConfirmedRef.current = false; });
    // Same pointer-before-touch ordering, other consumer: the long-press gate
    // reads placementMode at touchstart, after this confirm already cleared it,
    // so stamp the confirm to keep the hold from also opening a context menu.
    surveyLayerRef.current?.notePlacementConfirmed();

    // If there's a callback, call it with the result
    // The caller is responsible for what to do with the placement
    if (placement.onComplete) {
      placement.onComplete({
        position: placement.cursorPosition,
        snappedToStation: placement.snappedToStation,
      });
    }

    setPlacementCursor(null);
  }, [placementMode, confirmPlacement]);

  // Handle placement cancel
  const handlePlacementCancel = useCallback(() => {
    // Get the onCancel callback before canceling
    const onCancel = placementMode?.onCancel;
    cancelPlacementMode();
    setPlacementCursor(null);
    // Call the cancel callback if provided
    if (onCancel) {
      onCancel();
    }
  }, [placementMode, cancelPlacementMode]);

  // Clear snap when tool changes to non-snap tool
  useEffect(() => {
    const isSnapTool = activeTool === 'merge' || activeTool === 'measure' || activeTool === 'move_segment';
    if (!isSnapTool && snappedStation) {
      snappedStationRef.current = null;
      setSnappedStation(null);
    }
    // The measure shot-snap (#26) only exists for the measure tool - drop its
    // marker + state when leaving it so a stale cyan dot can't linger.
    if (activeTool !== 'measure' && (shotSnapMarkerRef.current || measureSnapShotRef.current)) {
      shotSnapMarkerRef.current?.remove();
      shotSnapMarkerRef.current = null;
      measureSnapShotRef.current = null;
      // measureSnapShot isn't subscribed (see useShallow above) - read it live.
      if (useUIStore.getState().measureSnapShot) setMeasureSnapShot(null);
    }
  }, [activeTool, snappedStation, setSnappedStation, setMeasureSnapShot]);

  // Tear down the shot-snap marker + ref whenever the store snap clears, even while
  // the measure tool stays active - a reset that keeps the tool (Escape, dialog,
  // geometry-mode/project change → clearMeasure) nulls the store snap but there's
  // no mousemove to run checkSnapToStation's teardown, so a stale cyan dot would
  // linger and the ref would desync from the cleared store value (#26, Codex).
  // Keyed on the null-ness boolean, so per-pixel `t` updates don't re-render here.
  const hasMeasureSnap = useUIStore((s) => s.measureSnapShot !== null);
  useEffect(() => {
    if (!hasMeasureSnap && (shotSnapMarkerRef.current || measureSnapShotRef.current)) {
      shotSnapMarkerRef.current?.remove();
      shotSnapMarkerRef.current = null;
      measureSnapShotRef.current = null;
    }
  }, [hasMeasureSnap]);

  // Hiding Shots disables shot interaction; hiding Jumps or Reference Shots
  // disables it for those legs. Drop a live measure shot-snap when its leg is no
  // longer drawn, so the cyan marker doesn't linger on hidden geometry - the
  // null-ness effect above then removes the marker + ref (#26, Codex).
  useEffect(() => {
    // Nothing is hidden -> nothing to drop. Bail before the lookup: reading
    // `section.shots` force-materializes the lazy per-section shot array
    // (`noteShotMaterialization` in loadProjectBinary.ts), and the default
    // all-visible state must not pay that on every project refresh.
    if (showShots && showJumps && showReferenceShots) return;
    const snap = useUIStore.getState().measureSnapShot;
    if (!snap) return;
    const kind = project?.sections
      .find((s) => s.id === snap.section_id)
      ?.shots.find((sh) => sh.id === snap.shot_id)?.kind;
    if (!isShotKindVisible(kind, showShots, showJumps, showReferenceShots)) {
      setMeasureSnapShot(null);
    }
  }, [showShots, showJumps, showReferenceShots, project?.sections, setMeasureSnapShot]);

  // Handle move drag start from SurveyLayer
  const handleMoveDragStart = useCallback(
    (data: { type: 'station' | 'segment' | 'shot'; sectionId?: number; stationRef?: { section_id: number; station_index: number }; shotIndex?: number; screenX?: number; screenY?: number }) => {
      if (!map.current) return;

      // Guard: when dragPan is disabled (move_segment tool), MapLibre's `click`
      // event fires even after long drags. After a snap-to-merge mouseup, the
      // click event hits the snap-target station and would start a spurious drag
      // for the target section. Block it while the merge dialog is pending.
      if (snapMergeDialogPendingRef.current) return;

      // Use actual click position if available, otherwise fall back to map center
      let startLng: number, startLat: number;
      if (data.screenX !== undefined && data.screenY !== undefined) {
        // Convert screen coordinates relative to map container
        const rect = map.current.getCanvas().getBoundingClientRect();
        const lngLat = map.current.unproject([data.screenX - rect.left, data.screenY - rect.top]);
        startLng = lngLat.lng;
        startLat = lngLat.lat;
      } else {
        const center = map.current.getCenter();
        startLng = center.lng;
        startLat = center.lat;
      }

      // Snap the grab baseline to the EXACT grabbed feature rather than the raw
      // unproject(click). At low zoom one pixel spans many meters, so the click
      // lands well off the feature and that offset only becomes visible once you
      // zoom in - anchoring on the feature's world coord makes IT (not the
      // imprecise click point) track the cursor through zoom (#79). A station grab
      // snaps to the station; a shot grab snaps to the nearest point on the shot.
      // NOTE: this baseline reads `activeStations` (geometry-mode-aware), while the
      // per-frame handleMoveDragMove and the commit read raw `stations`. They diverge
      // only in `closureAdjusted` mode - but move tools are mutating, so the tool is
      // forced back to `select` there (isMutatingTool effect), making the move-drag
      // path unreachable in that mode. If that disable is ever relaxed, switch this to
      // `stations` so the baseline and per-frame origin share one coordinate set.
      if (data.stationRef) {
        const st = activeStations?.find(
          (s) =>
            s.station_ref.section_id === data.stationRef!.section_id &&
            s.station_ref.station_index === data.stationRef!.station_index,
        );
        if (st) {
          const mc = metersToMapCoords(st.coordinates.x, st.coordinates.y, gpsOrigin);
          startLng = mc[0];
          startLat = mc[1];
        }
      } else if (surveyLayerRef.current && data.screenX !== undefined && data.screenY !== undefined) {
        // Shot-based grab (move_shot, or a segment grabbed by clicking a shot line).
        const rect = map.current.getCanvas().getBoundingClientRect();
        const snap = surveyLayerRef.current.queryNearestShotPoint(
          data.screenX - rect.left,
          data.screenY - rect.top,
          config?.map?.shot?.hit_area ?? 8,
        );
        if (snap) {
          startLng = snap.lng;
          startLat = snap.lat;
        }
      }

      const baseDragState: MoveDragState = {
        type: data.type,
        startLngLat: { lng: startLng, lat: startLat },
        currentLngLat: { lng: startLng, lat: startLat },
      };

      // Initialize hot-path refs for drag tracking
      moveDragLngLatRef.current = { lng: startLng, lat: startLat };
      // Seed the last-cursor ref so a zoom before the first move re-applies correctly.
      lastCursorClientRef.current = (data.screenX !== undefined && data.screenY !== undefined)
        ? { clientX: data.screenX, clientY: data.screenY }
        : null;

      // Build the drag state conditionally to avoid undefined values
      if (data.type === 'station' && data.stationRef !== undefined) {
        // Guard: block drag on immovable stations (zero-datum or transitively connected)
        const stationKey = `${data.stationRef.section_id}:${data.stationRef.station_index}`;
        if (immovableStationKeys.has(stationKey)) return;

        // Store origin station for snap-to-merge during drag
        dragOriginStationRef.current = data.stationRef;
        setMoveDrag({ ...baseDragState, type: 'station', stationRef: data.stationRef });
        // Begin drag preview - returns co-located stations (merged/closured at same coords)
        const colocated = surveyLayerRef.current?.beginStationDragPreview(
          data.stationRef.section_id,
          data.stationRef.station_index
        ) ?? [];
        stationDragColocatedRefsRef.current = colocated;
      } else if (data.type === 'segment' && data.sectionId !== undefined) {
        const segmentSectionId = data.sectionId;
        // Always clear selection on any segment click, even if drag is blocked
        clearSelectionAndVisuals();
        // Store origin station for snap-to-merge during drag
        dragOriginStationRef.current = data.stationRef ?? null;
        // Block segment drag until anchored IDs have been fetched (prevents
        // moving anchored segments during the window after tool activation)
        if (!anchoredIdsLoaded) return;
        // Check if section is anchored - silent no-op if so
        if (anchoredSectionIds.has(segmentSectionId)) return;

        // Set a pending drag token to detect stale async results
        const dragToken = ++segmentDragTokenRef.current;

        // Fetch connected section IDs at drag start
        getConnectedSectionIds(segmentSectionId).then((ids) => {
          // Discard late results if a newer drag started or drag was cancelled
          if (segmentDragTokenRef.current !== dragToken) return;

          // Filter out anchored sections
          const movable = ids.filter((id) => !anchoredSectionIds.has(id));
          const connectedSet = new Set(movable);
          dragConnectedSectionIdsRef.current = connectedSet;
          // Clear segment hover before starting drag preview (temp layers take over)
          surveyLayerRef.current?.setHoveredSegment([]);
          // Begin drag preview - creates temp layers with *-translate paint property
          surveyLayerRef.current?.beginDragPreview(connectedSet);
          setMoveDrag({
            ...baseDragState,
            type: 'segment',
            sectionId: segmentSectionId,
            connectedSectionIds: movable,
          });
        }).catch(() => {
          // Discard late results if a newer drag started or drag was cancelled
          if (segmentDragTokenRef.current !== dragToken) return;

          // On failure, just move the single section
          const fallbackSet = new Set([segmentSectionId]);
          dragConnectedSectionIdsRef.current = fallbackSet;
          surveyLayerRef.current?.setHoveredSegment([]);
          surveyLayerRef.current?.beginDragPreview(fallbackSet);
          setMoveDrag({
            ...baseDragState,
            type: 'segment',
            sectionId: segmentSectionId,
            connectedSectionIds: [segmentSectionId],
          });
        });
      } else if (data.type === 'shot' && data.sectionId !== undefined && data.shotIndex !== undefined) {
        // Guard: block drag if either endpoint is immovable
        const fromKey = `${data.sectionId}:${data.shotIndex}`;
        const toKey = `${data.sectionId}:${data.shotIndex + 1}`;
        if (immovableStationKeys.has(fromKey) || immovableStationKeys.has(toKey)) return;

        setMoveDrag({
          ...baseDragState,
          type: 'shot',
          sectionId: data.sectionId,
          shotIndex: data.shotIndex,
        });
        // Begin drag preview seeded from both shot endpoints - returns all co-located stations
        // (merged/closured at same coordinates) that should move together
        const otherEndpoint = { section_id: data.sectionId, station_index: data.shotIndex + 1 };
        const colocated = surveyLayerRef.current?.beginStationDragPreview(
          data.sectionId, data.shotIndex, [otherEndpoint]
        ) ?? [];
        stationDragColocatedRefsRef.current = colocated;
      }
    },
    [anchoredSectionIds, anchoredIdsLoaded, clearSelectionAndVisuals, getConnectedSectionIds, immovableStationKeys, activeStations, gpsOrigin, config?.map?.shot?.hit_area]
  );

  // Get map tool callbacks
  // Re-query the measure snap fresh at a screen point (stations win, else a shot
  // point), respecting snap config + layer visibility. Used at click time so the
  // committed endpoint matches the actual click, not the RAF/deferred snap store (#26).
  const querySnapAtPoint = useCallback(
    (screenX: number, screenY: number) => {
      const snapConfig = config?.map.snap;
      if (!snapConfig?.enabled_measure || !surveyLayerRef.current) return null;
      const station = surveyLayerRef.current.queryNearestStation(screenX, screenY, snapConfig.threshold);
      if (station) return { kind: 'station' as const, station };
      const shot = surveyLayerRef.current.queryNearestShotPoint(screenX, screenY, snapConfig.threshold);
      return shot
        ? { kind: 'shot' as const, section_id: shot.section_id, shot_id: shot.shot_id, t: shot.t }
        : null;
    },
    [config?.map.snap]
  );

  const baseToolCallbacks = useMapTools({
    onMoveDragStart: handleMoveDragStart,
    onFeatureHover: setIsHoveringFeature,
    immovableStationKeys,
    gpsOrigin,
    querySnapAtPoint,
  });

  const noteMapInteraction = useCallback(() => {
    mapInteractionSequenceRef.current += 1;
  }, []);

  // Combine with hidden feature callbacks for toast feedback
  const toolCallbacks = useMemo<SurveyLayerCallbacks>(
    () => ({
      ...baseToolCallbacks,
      onStationClick: (ref, event) => {
        noteMapInteraction();
        baseToolCallbacks.onStationClick?.(ref, event);
      },
      onShotClick: (sectionId, shotId, event) => {
        noteMapInteraction();
        baseToolCallbacks.onShotClick?.(sectionId, shotId, event);
      },
      onEmptyClick: (event) => {
        noteMapInteraction();
        baseToolCallbacks.onEmptyClick?.(event);
      },
      onContextMenu: (target, event) => {
        noteMapInteraction();
        baseToolCallbacks.onContextMenu?.(target, event);
      },
      onHiddenStationClick: () => {
        noteMapInteraction();
        showToast(t('map.toasts.stationsHidden'), TOAST_KEYS.STATIONS_HIDDEN, 'warning');
      },
      onHiddenShotClick: () => {
        noteMapInteraction();
        showToast(t('map.toasts.shotsHidden'), TOAST_KEYS.SHOTS_HIDDEN, 'warning');
      },
      // No stock fallback: surface a subsystem-specific toast (renderFatalMessage).
      onRenderFatal: (reason: string) => showToast(renderFatalMessage(reason), TOAST_KEYS.RENDER_FATAL, 'error'),
    }),
    [baseToolCallbacks, noteMapInteraction, showToast],
  );

  // Zoom box handlers (for click-to-zoom and box zoom in task 3)
  const handleZoomBoxStart = useCallback(
    (e: React.PointerEvent) => {
      if (activeTool !== 'zoom') return;
      if (!mapContainer.current) return;

      // Only start on left click, not when Ctrl is held (Ctrl+drag is for panning)
      if (e.button !== 0 || e.ctrlKey) return;

      const rect = mapContainer.current.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;

      setZoomBox({ start: { x, y }, end: { x, y } });
    },
    [activeTool]
  );

  const handleZoomBoxMove = useCallback(
    (e: React.PointerEvent) => {
      if (!zoomBox.start || !mapContainer.current) return;
      // Only update if primary button is still pressed (prevents stuck state on focus loss)
      if (e.buttons !== 1) return;

      const rect = mapContainer.current.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;

      setZoomBox((prev) => ({ ...prev, end: { x, y } }));
    },
    [zoomBox.start]
  );

  const handleZoomBoxEnd = useCallback(() => {
    if (!zoomBox.start || !zoomBox.end || !map.current) {
      setZoomBox({ start: null, end: null });
      return;
    }

    const minX = Math.min(zoomBox.start.x, zoomBox.end.x);
    const maxX = Math.max(zoomBox.start.x, zoomBox.end.x);
    const minY = Math.min(zoomBox.start.y, zoomBox.end.y);
    const maxY = Math.max(zoomBox.start.y, zoomBox.end.y);

    // For small drags (<10px), treat as click-to-zoom
    if (maxX - minX < 10 || maxY - minY < 10) {
      // Click zoom-in at center of box (effectively the click point)
      const centerX = (minX + maxX) / 2;
      const centerY = (minY + maxY) / 2;
      const clickLngLat = map.current.unproject([centerX, centerY]);

      const currentZoom = map.current.getZoom();
      const targetZoom = Math.min(zoomMax, currentZoom + zoomStep);

      if (targetZoom !== currentZoom) {
        map.current.easeTo({
          center: clickLngLat,
          zoom: targetZoom,
          duration: 300,
        });
      }
    } else {
      // Large drag: box zoom to bounds
      // Note: Y is inverted in screen coords (top=0)
      const sw = map.current.unproject([minX, maxY]); // bottom-left screen = SW
      const ne = map.current.unproject([maxX, minY]); // top-right screen = NE

      map.current.fitBounds([sw, ne], {
        padding: 20,
        linear: true,
        duration: 300,
        maxZoom: zoomMax,
      });
    }

    setZoomBox({ start: null, end: null });
  }, [zoomBox, zoomStep, zoomMax]);

  // Move drag handlers - hot path bypasses React state for performance.
  // Position is stored in a ref (no re-render), and segment preview is updated
  // synchronously since setPaintProperty is essentially free (shader uniform).
  const handleMoveDragMove = useCallback(
    // skipSnapScan: the per-frame map-'move' re-apply (zoom/pan) passes true to skip the
    // O(all-stations) snap SCAN - the cursor's world position is unchanged so the snap
    // target can't change. The cheap snap RE-ALIGN (for an already-held target) and the
    // #79 re-projection still run, so the preview tracks and stays snapped across zoom.
    (clientX: number, clientY: number, skipSnapScan = false) => {
      // While the snap-merge dialog is up the drag is paused (moveDrag is held alive
      // so Cancel can resume it) - don't let the map-'move' listener re-apply the
      // preview and clobber the held position. Mirrors handleMoveDragStart/End, which
      // both already guard on this; this is the only drag-move entry that didn't.
      if (snapMergeDialogPendingRef.current) return;
      if (!moveDrag || !map.current || !mapContainer.current) return;

      const rect = mapContainer.current.getBoundingClientRect();
      const x = clientX - rect.left;
      const y = clientY - rect.top;

      // Convert screen coords to map coords
      const lngLat = map.current.unproject([x, y]);

      // Store in ref (no React state update - avoids re-render per mousemove)
      moveDragLngLatRef.current = { lng: lngLat.lng, lat: lngLat.lat };
      lastCursorClientRef.current = { clientX, clientY };

      // Re-project the grab's WORLD point at the CURRENT zoom (NOT a fixed grab-screen
      // baseline - that drifts when the map re-projects on zoom mid-drag, #79). dx/dy
      // is then the cursor offset from where the grabbed point renders right now.
      const grabScreen = map.current.project([moveDrag.startLngLat.lng, moveDrag.startLngLat.lat]);
      const dx = x - grabScreen.x;
      const dy = y - grabScreen.y;
      lastRawDragOffsetRef.current = { dx, dy };

      // Station drag preview - translate station circles/labels + rebuild shot geometries
      if (moveDrag.type === 'station' && surveyLayerRef.current) {
        const snapThreshold = config?.map.snap.threshold ?? 10;
        // Enter for a live scan, OR (on the zoom path) only to re-align a held snap.
        if ((!skipSnapScan || dragSnapRef.current) && dragOriginStationRef.current && snapThreshold > 0) {
          const originSt = stations.find(
            (s) =>
              s.station_ref.section_id === dragOriginStationRef.current!.section_id &&
              s.station_ref.station_index === dragOriginStationRef.current!.station_index
          );
          if (originSt) {
            const originMapCoords = metersToMapCoords(originSt.coordinates.x, originSt.coordinates.y, gpsOrigin);
            const originScreen = map.current.project(originMapCoords);

            let nearestRef: StationRef | null = dragSnapRef.current;
            if (!skipSnapScan) {
              const projectedX = originScreen.x + dx;
              const projectedY = originScreen.y + dy;

              // Find nearest non-co-located station to the projected origin position
              nearestRef = null;
              let nearestDist = Infinity;
              const colocatedKeys = new Set(
                stationDragColocatedRefsRef.current.map(r => `${r.section_id}:${r.station_index}`)
              );
              for (const st of stations) {
                const key = `${st.station_ref.section_id}:${st.station_ref.station_index}`;
                if (colocatedKeys.has(key)) continue;
                const mc = metersToMapCoords(st.coordinates.x, st.coordinates.y, gpsOrigin);
                const sp = map.current!.project(mc);
                const sdx = sp.x - projectedX;
                const sdy = sp.y - projectedY;
                const dist = Math.sqrt(sdx * sdx + sdy * sdy);
                if (dist < snapThreshold && dist < nearestDist) {
                  nearestDist = dist;
                  nearestRef = st.station_ref;
                }
              }

              // Update snap state only when target changes (avoids React churn)
              const prev = dragSnapRef.current;
              if (
                nearestRef?.section_id !== prev?.section_id ||
                nearestRef?.station_index !== prev?.station_index
              ) {
                dragSnapRef.current = nearestRef;
                setSnappedStation(nearestRef);
              }
            }

            // If snapped, align the origin station onto the target. Recomputed at the
            // CURRENT zoom (O(1)), so the snap holds through a mid-drag zoom even when
            // the scan above was skipped.
            if (nearestRef) {
              const targetSt = stations.find(
                (s) =>
                  s.station_ref.section_id === nearestRef!.section_id &&
                  s.station_ref.station_index === nearestRef!.station_index
              );
              if (targetSt) {
                const targetMapCoords = metersToMapCoords(targetSt.coordinates.x, targetSt.coordinates.y, gpsOrigin);
                const targetScreen = map.current.project(targetMapCoords);
                const snapDx = targetScreen.x - originScreen.x;
                const snapDy = targetScreen.y - originScreen.y;
                surveyLayerRef.current.updateStationDragPreview(snapDx, snapDy);
                return;
              }
            }
          }
        }

        // No snap (or no origin station): use raw offset
        surveyLayerRef.current.updateStationDragPreview(dx, dy);
      }

      // Shot drag preview - same pipeline as station drag (both endpoints + co-located stations move)
      if (moveDrag.type === 'shot' && surveyLayerRef.current) {
        surveyLayerRef.current.updateStationDragPreview(dx, dy);
      }

      // For segment drags, update preview synchronously (no rAF needed). dx/dy comes
      // from the re-projected grab point above, so the *-translate (viewport anchor)
      // tracks the cursor across zoom changes mid-drag (#79).
      if (moveDrag.type === 'segment' && surveyLayerRef.current) {
        // Snap-to-station detection during segment drag (only if drag started from a station)
        const snapThreshold = config?.map.snap.threshold ?? 10;
        // Enter for a live scan, OR (on the zoom path) only to re-align a held snap.
        if ((!skipSnapScan || dragSnapRef.current) && dragOriginStationRef.current && snapThreshold > 0) {
          // Find origin station's screen position and apply drag offset
          const originSt = stations.find(
            (s) =>
              s.station_ref.section_id === dragOriginStationRef.current!.section_id &&
              s.station_ref.station_index === dragOriginStationRef.current!.station_index
          );
          if (originSt) {
            const originMapCoords = metersToMapCoords(originSt.coordinates.x, originSt.coordinates.y, gpsOrigin);
            const originScreen = map.current.project(originMapCoords);

            let nearestRef: StationRef | null = dragSnapRef.current;
            if (!skipSnapScan) {
              const projectedX = originScreen.x + dx;
              const projectedY = originScreen.y + dy;

              // Find nearest non-segment station to the projected origin position
              nearestRef = null;
              let nearestDist = Infinity;
              const connectedIds = dragConnectedSectionIdsRef.current;
              for (const st of stations) {
                if (connectedIds.has(st.station_ref.section_id)) continue;
                const mc = metersToMapCoords(st.coordinates.x, st.coordinates.y, gpsOrigin);
                const sp = map.current!.project(mc);
                const sdx = sp.x - projectedX;
                const sdy = sp.y - projectedY;
                const dist = Math.sqrt(sdx * sdx + sdy * sdy);
                if (dist < snapThreshold && dist < nearestDist) {
                  nearestDist = dist;
                  nearestRef = st.station_ref;
                }
              }

              // Update snap state only when target changes (avoids React churn)
              const prev = dragSnapRef.current;
              if (
                nearestRef?.section_id !== prev?.section_id ||
                nearestRef?.station_index !== prev?.station_index
              ) {
                dragSnapRef.current = nearestRef;
                setSnappedStation(nearestRef);
              }
            }

            // If snapped, align the origin station onto the target. Recomputed at the
            // CURRENT zoom (O(1)), so the snap holds through a mid-drag zoom even when
            // the scan above was skipped.
            if (nearestRef) {
              const targetSt = stations.find(
                (s) =>
                  s.station_ref.section_id === nearestRef!.section_id &&
                  s.station_ref.station_index === nearestRef!.station_index
              );
              if (targetSt) {
                const targetMapCoords = metersToMapCoords(targetSt.coordinates.x, targetSt.coordinates.y, gpsOrigin);
                const targetScreen = map.current.project(targetMapCoords);
                const snapDx = targetScreen.x - originScreen.x;
                const snapDy = targetScreen.y - originScreen.y;
                surveyLayerRef.current.updateDragPreview(snapDx, snapDy);
                return;
              }
            }
          }
        }

        // No snap (or no origin station): use raw offset
        surveyLayerRef.current.updateDragPreview(dx, dy);
      }
    },
    [moveDrag, config, stations, gpsOrigin, setSnappedStation]
  );

  // While a move-drag is active, re-apply the preview on every map move (zoom OR
  // programmatic pan) so the grabbed feature stays glued to the cursor - a
  // scroll-zoom / easeTo fires no pointer event, so synthesize from the last cursor
  // position (#79). No-op when lastCursorClientRef is null: a center-grab with no
  // screen coords forgoes zoom-tracking (every real caller passes coords).
  useEffect(() => {
    if (!moveDrag || !map.current) return;
    const m = map.current;
    const onMapMove = () => {
      const c = lastCursorClientRef.current;
      // skipSnapScan=true: this fires per-frame during zoom/pan; only re-project +
      // re-align (cheap), never the O(all-stations) snap scan.
      if (c) handleMoveDragMove(c.clientX, c.clientY, true);
    };
    m.on('move', onMapMove);
    return () => {
      m.off('move', onMapMove);
    };
  }, [moveDrag, handleMoveDragMove]);

  // After a snap-to-merge drag (station or segment), check connectivity and open merge/closure dialog.
  // Separate from checkAndOpenMergeDialog in useMapTools because the active tool is a
  // move tool (not merge) and we don't set mergeStationA/B in the UI store.
  const promptSnapMerge = useCallback(
    async (
      originStation: StationRef,
      targetStation: StationRef,
      moveCallbacks?: {
        onBeforeConfirm: () => Promise<void>;
        onCancel: () => void;
        preCommands?: BatchCommand[];
      }
    ) => {
      const { checkMergeConnectivity } = useProjectStore.getState();
      const { openDialog, showToast } = useUIStore.getState();

      showToast(t('map.toasts.mergeChecking'), TOAST_KEYS.MERGE_CHECK, 'info');
      try {
        const result = await checkMergeConnectivity(originStation, targetStation);
        const extra = moveCallbacks
          ? { onBeforeConfirm: moveCallbacks.onBeforeConfirm, onCancel: moveCallbacks.onCancel, preCommands: moveCallbacks.preCommands }
          : {};
        if (result.can_merge) {
          openDialog('merge', { stationA: originStation, stationB: targetStation, ...extra });
        } else if (result.connected || result.reason === 'Both sections have fixed coordinates') {
          openDialog('closure', {
            stationA: originStation,
            stationB: targetStation,
            preview: result.preview,
            reason: result.reason,
            ...extra,
          });
        } else {
          showToast(result.reason ?? t('map.toasts.mergeNotPossible'), TOAST_KEYS.MERGE_BLOCKED, 'warning');
          moveCallbacks?.onCancel?.();
        }
      } catch (e) {
        showToast(getErrorMessage(e), TOAST_KEYS.MERGE_ERROR, 'error');
        moveCallbacks?.onCancel?.();
      }
    },
    []
  );

  const handleMoveDragEnd = useCallback(async () => {
    // Block while snap-to-merge dialog is pending. The guard in handlePointerUp
    // should already prevent this call, but act as defense-in-depth in case
    // the mouseup reaches us through an unexpected path (e.g., portal event
    // propagation, inert removal timing).
    if (snapMergeDialogPendingRef.current) {
      return;
    }

    // Invalidate any pending getConnectedSectionIds async results so late
    // .then() callbacks cannot call setMoveDrag after drag has ended.
    segmentDragTokenRef.current++;

    // Capture snap refs
    const snapTarget = dragSnapRef.current;
    const originStationRef = dragOriginStationRef.current;
    dragSnapRef.current = null;

    if (!moveDrag || !map.current) {
      // Early bail - clean up everything
      snapMergeDialogPendingRef.current = false;
      dragOriginStationRef.current = null;
      lastRawDragOffsetRef.current = null;
      stationDragColocatedRefsRef.current = [];
      if (surveyLayerRef.current) surveyLayerRef.current.endDragPreview();
      setMoveDrag(null);
      dragConnectedSectionIdsRef.current = new Set();
      moveDragLngLatRef.current = null;
      lastCursorClientRef.current = null;
      if (snapTarget) setSnappedStation(null);
      return;
    }

    // Snap-to-merge path: keep the entire drag state alive so that Cancel
    // simply dismisses the dialog and the user can continue dragging.
    if (moveDrag.type === 'segment' && moveDrag.sectionId !== undefined && snapTarget && originStationRef) {
      const originSt = stations.find(
        (s) =>
          s.station_ref.section_id === originStationRef.section_id &&
          s.station_ref.station_index === originStationRef.station_index
      );
      const targetSt = stations.find(
        (s) =>
          s.station_ref.section_id === snapTarget.section_id &&
          s.station_ref.station_index === snapTarget.station_index
      );

      if (originSt && targetSt) {
        const snapOffset = {
          x: targetSt.coordinates.x - originSt.coordinates.x,
          y: targetSt.coordinates.y - originSt.coordinates.y,
          z: 0,
        };
        const sectionId = moveDrag.sectionId;
        setSnappedStation(null);
        snapMergeDialogPendingRef.current = true;

        // Build move command for batching with merge/closure (single undo entry)
        const segmentMoveCmd: BatchCommand = { type: 'move_segment', section_id: sectionId, offset: snapOffset };

        // Don't clear moveDrag, dragConnectedSectionIdsRef, or dragOriginStationRef
        // - these keep the drag alive after Cancel.
        promptSnapMerge(originStationRef, snapTarget, {
          preCommands: [segmentMoveCmd],
          onBeforeConfirm: async () => {
            // UI cleanup only - backend move is batched with merge/closure by the dialog
            snapMergeDialogPendingRef.current = false;
            dragOriginStationRef.current = null;
            lastRawDragOffsetRef.current = null;
            surveyLayerRef.current?.onNextUpdate(() => {
              surveyLayerRef.current?.endDragPreview();
            });
            // Clear the overlay follow-offset synchronously (see the non-snap
            // segment branch) so the post-merge rebuild never doubles the offset.
            surveyLayerRef.current?.commitDragPreview();
            dragConnectedSectionIdsRef.current = new Set();
            moveDragLngLatRef.current = null;
            lastCursorClientRef.current = null;
            setMoveDrag(null);
          },
          onCancel: () => {
            // Resume drag: restore preview to the raw (pre-snap) offset so the
            // segment appears where the user's mouse was, not at the snap target.
            const raw = lastRawDragOffsetRef.current;
            if (raw && surveyLayerRef.current) {
              surveyLayerRef.current.updateDragPreview(raw.dx, raw.dy);
            }
            snapMergeDialogPendingRef.current = false;
          },
        });
        return;
      }
    }

    // Snap-to-merge path for station drag: keep drag state alive so Cancel resumes drag.
    if (moveDrag.type === 'station' && moveDrag.stationRef && snapTarget && originStationRef) {
      const originSt = stations.find(
        (s) =>
          s.station_ref.section_id === originStationRef.section_id &&
          s.station_ref.station_index === originStationRef.station_index
      );
      const targetSt = stations.find(
        (s) =>
          s.station_ref.section_id === snapTarget.section_id &&
          s.station_ref.station_index === snapTarget.station_index
      );

      if (originSt && targetSt) {
        const snapOffset = {
          x: targetSt.coordinates.x - originSt.coordinates.x,
          y: targetSt.coordinates.y - originSt.coordinates.y,
          z: 0,
        };
        setSnappedStation(null);
        snapMergeDialogPendingRef.current = true;

        // Build move commands for batching with merge/closure (single undo entry)
        const colocated = stationDragColocatedRefsRef.current;
        const moveCommands: BatchCommand[] = colocated.map(ref => ({
          type: 'move_station' as const, station_ref: ref, offset: snapOffset,
        }));

        // Don't clear moveDrag or dragOriginStationRef -
        // these keep the drag alive after Cancel.
        promptSnapMerge(originStationRef, snapTarget, {
          preCommands: moveCommands,
          onBeforeConfirm: async () => {
            // UI cleanup only - backend move is batched with merge/closure by the dialog
            snapMergeDialogPendingRef.current = false;
            dragOriginStationRef.current = null;
            lastRawDragOffsetRef.current = null;
            stationDragColocatedRefsRef.current = [];
            surveyLayerRef.current?.onNextUpdate(() => {
              surveyLayerRef.current?.endDragPreview();
            });
            surveyLayerRef.current?.commitDragPreview();
            moveDragLngLatRef.current = null;
            lastCursorClientRef.current = null;
            setMoveDrag(null);
          },
          onCancel: () => {
            // Resume drag: restore preview to the raw (pre-snap) offset so the
            // station appears where the user's mouse was, not at the snap target.
            const raw = lastRawDragOffsetRef.current;
            if (raw && surveyLayerRef.current) {
              surveyLayerRef.current.updateStationDragPreview(raw.dx, raw.dy);
            }
            snapMergeDialogPendingRef.current = false;
          },
        });
        return;
      }
    }

    // --- Normal (non-snap) path: clear everything and commit ---
    dragOriginStationRef.current = null;
    lastRawDragOffsetRef.current = null;
    dragConnectedSectionIdsRef.current = new Set();

    // Read final position from ref (updated imperatively during drag)
    const endLngLat = moveDragLngLatRef.current ?? moveDrag.currentLngLat;
    moveDragLngLatRef.current = null;
    lastCursorClientRef.current = null;

    // Convert drag start/end from map lng/lat to meters, then compute delta
    const startMeters = mapCoordsToMeters(moveDrag.startLngLat.lng, moveDrag.startLngLat.lat, gpsOrigin);
    const endMeters = mapCoordsToMeters(endLngLat.lng, endLngLat.lat, gpsOrigin);
    const dx = endMeters.x - startMeters.x;
    const dy = endMeters.y - startMeters.y;

    // Only process if there's a meaningful movement (1mm threshold)
    if (Math.abs(dx) > 0.001 || Math.abs(dy) > 0.001) {
      const offset = { x: dx, y: dy, z: 0 };

      if (moveDrag.type === 'station' && moveDrag.stationRef) {
        // Commit preview: remove temp layers but keep exclusion filters so
        // features stay hidden until React re-renders with new coordinates.
        surveyLayerRef.current?.onNextUpdate(() => {
          surveyLayerRef.current?.endDragPreview();
        });
        surveyLayerRef.current?.commitDragPreview();

        // Move all co-located stations (merged/closured at same coordinates)
        const colocated = stationDragColocatedRefsRef.current;
        stationDragColocatedRefsRef.current = [];
        const res = colocated.length > 1
          ? await executeBatch(
              colocated.map(ref => ({ type: 'move_station' as const, station_ref: ref, offset }))
            )
          : await moveStation(moveDrag.stationRef, offset);
        // If the move was rejected (e.g. the #191 read-only edit gate), the
        // scheduled endDragPreview only fires on the next layer update after a
        // successful refresh - which never comes - so the committed preview would
        // leave the dragged features hidden ("shots vanish"). Restore them now.
        if (!res.success) surveyLayerRef.current?.endDragPreview();
        setMoveDrag(null);
        return;
      } else if (moveDrag.type === 'segment' && moveDrag.sectionId !== undefined) {
        // Non-snap segment move
        surveyLayerRef.current?.onNextUpdate(() => {
          surveyLayerRef.current?.endDragPreview();
        });
        // Commit preview NOW (mirrors the station/shot branches): removes the
        // temp layers and clears the overlay follow-offset synchronously so the
        // post-commit interact-overlay rebuild reads the new coords WITHOUT the
        // stale drag offset. Without this the highlight briefly renders at
        // newPosition + dragOffset (a one-frame double-distance flash).
        surveyLayerRef.current?.commitDragPreview();
        const res = await moveSegment(moveDrag.sectionId, offset);
        // Rejected move (read-only gate): restore now - see the station branch.
        if (!res.success) surveyLayerRef.current?.endDragPreview();
        setMoveDrag(null);
        return;
      } else if (moveDrag.type === 'shot' && moveDrag.sectionId !== undefined && moveDrag.shotIndex !== undefined) {
        // Commit preview: remove temp layers but keep exclusion filters so
        // features stay hidden until React re-renders with new coordinates.
        surveyLayerRef.current?.onNextUpdate(() => {
          surveyLayerRef.current?.endDragPreview();
        });
        surveyLayerRef.current?.commitDragPreview();

        // Move all co-located stations (includes both shot endpoints + merged/closured)
        const colocated = stationDragColocatedRefsRef.current;
        stationDragColocatedRefsRef.current = [];
        const res = await executeBatch(
          colocated.map(ref => ({ type: 'move_station' as const, station_ref: ref, offset }))
        );
        // Rejected move (read-only gate): restore the hidden features now, since
        // the success-only refresh that would fire endDragPreview never runs.
        if (!res.success) surveyLayerRef.current?.endDragPreview();
        setMoveDrag(null);
        return;
      }
    }

    // For non-segment drags or no meaningful movement, end preview immediately
    // (no-op if beginDragPreview was never called)
    stationDragColocatedRefsRef.current = [];
    if (surveyLayerRef.current) surveyLayerRef.current.endDragPreview();
    setMoveDrag(null);
  }, [moveDrag, moveStation, moveSegment, executeBatch, gpsOrigin, stations, promptSnapMerge, setSnappedStation]);

  // Initialize map
  useEffect(() => {
    if (!mapContainer.current || map.current) return;

    // Calculate initial center from stations (in meters). Read the SoA coord
    // buffer directly so load does not materialize the whole ~89k Station graph
    // through the lazy proxy just to size the initial view (#mem).
    const bounds = getBoundsFromStationCoords(surveyData?.stationCoords ?? new Float64Array(0));
    const centerX = (bounds.minX + bounds.maxX) / 2;
    const centerY = (bounds.minY + bounds.maxY) / 2;
    // Transform to map coordinates
    const mapCenter = metersToMapCoords(centerX, centerY, gpsOrigin);

    const created = createMap({
      container: mapContainer.current,
      style: {
        version: 8,
        // Use OpenMapTiles fonts for text labels
        glyphs: 'https://fonts.openmaptiles.org/{fontstack}/{range}.pbf',
        sources: {},
        layers: [
          {
            id: 'background',
            type: 'background',
            paint: {
              'background-color': getMapThemeColors().background,
            },
          },
        ],
      },
      center: mapCenter,
      // Read at creation time rather than subscribed: this effect has an empty
      // dep array, so a subscription could never deliver a newer value here and
      // would only re-render MapCanvas on every 'zoomend' (#430).
      zoom: useUIStore.getState().mapZoom,
      attributionControl: false,
      // Disable built-in keyboard zoom (handled by useKeyboardShortcuts with config.map.zoom_step)
      keyboard: false,

      // Performance optimizations
      // See: https://deepwiki.com/maplibre/maplibre-gl-js/5.2-performance-optimization-techniques
      maxTileCacheSize: 100, // Limit tile cache memory
      maxTileCacheZoomLevels: 5, // Limit zoom range caching
      fadeDuration: 0, // Disable fade animations for snappier response
      renderWorldCopies: false, // Not needed for cave maps (saves render work)

      // WebGL context optimizations (MapLibre v5+)
      // Type assertion needed: canvasContextAttributes is supported at runtime
      // but not yet in the MapLibre GL JS TypeScript definitions
      ...({
        canvasContextAttributes: {
          antialias: false, // Disable AA for better performance
          preserveDrawingBuffer: false, // Allow buffer discard
          failIfMajorPerformanceCaveat: false, // Don't fail on software renderer
        },
      } as Record<string, unknown>),
    });
    if (!created) {
      setWebgl2Unavailable(true);
      return;
    }
    map.current = created;

    // Optional MapLibre instrumentation; inert unless VITE_DEBUG_MACOS_LOCKUP
    // is set at build time. See docs/debugging-macos-pan-zoom.md.
    installPanZoomDebug(map.current, {
      customPanActive: customPanActiveRef,
      ctrlPanActive: ctrlPanActiveRef,
      placementJustConfirmed: placementJustConfirmedRef,
      ctrlPanMouseEventTime: ctrlPanMouseEventTimeRef,
    });

    // Disable rotation (Ctrl+drag rotates by default, we want Ctrl+drag to pan)
    map.current.dragRotate.disable();
    map.current.touchPitch.disable();
    // Two-finger twist rotates by default; the survey is a top-down plan, so a
    // pinch must zoom only.
    map.current.touchZoomRotate.disableRotation();
    // Disable box zoom (Ctrl+drag draws selection box by default, we want Ctrl+drag to pan)
    map.current.boxZoom.disable();

    // Add navigation controls
    map.current.addControl(
      new maplibregl.NavigationControl({ showCompass: false }),
      'top-right'
    );

    // Create the scale control instance (always shown; added once on map load).
    scaleControlRef.current = new maplibregl.ScaleControl({ maxWidth: 100, unit: 'metric' });

    // Live map scale for the to-scale placement preview. <PlacementOverlay> is
    // the only consumer and mounts only while placement mode is active, so
    // outside placement this frame-rate handler must not touch React state
    // (#430). The flag is read from the store rather than from a captured value
    // on purpose: this effect has an empty dep array, so adding placement mode
    // to the deps would tear the map down and recreate it on every toggle.
    map.current.on('zoom', () => {
      if (!map.current) return;
      if (useUIStore.getState().placementMode === null) return;
      setPixelsPerMeter(pixelsPerMeterAt(map.current));
    });

    // Camera zoom is mirrored into the store once per gesture, not per frame.
    // NOTE: `mapZoom` in the store is therefore STALE mid-gesture. Its only
    // reader is the initial-camera restore above (the `zoom:` option), which runs
    // when the map is recreated. Any future live-zoom consumer must subscribe
    // to the map (`map.on('zoom')`), never to the store.
    map.current.on('zoomend', () => {
      if (!map.current) return;
      setMapZoom(map.current.getZoom());
    });

    // Initialize survey layer
    map.current.on('load', () => {
      if (!map.current) return;
      surveyLayerRef.current = new SurveyLayer(map.current, toolCallbacks);
      // Set initial tool immediately after creating SurveyLayer
      surveyLayerRef.current.setActiveTool(useUIStore.getState().activeTool);
      setMapLoaded(true);
      // pixelsPerMeter is seeded when placement mode activates, not here - see
      // the placement-scale effect.
    });

    return () => {
      // [perf] Time the MapCanvas teardown so we can isolate map.remove()
      // cost vs React unmount cost on the back-button freeze. Surfaced in
      // the MemoryProbe panel.
      const t0 = performance.now();
      // 'zoomend' never fires if we unmount mid-gesture, so capture the final
      // zoom here - otherwise the restore path would come back one wheel chain
      // behind.
      if (map.current) setMapZoom(map.current.getZoom());
      if (snapRafId.current !== null) {
        cancelAnimationFrame(snapRafId.current);
        snapRafId.current = null;
      }
      // map.remove() tears down all layers, sources, the worker and the
      // canvas element atomically - calling surveyLayer.destroy() first
      // duplicates that work serially while the worker still holds a large
      // FeatureCollection, which blocks the main thread for seconds on a
      // 75k-shot survey (the "back-button is frozen" symptom). We rely on
      // the pending-update microtask in SurveyLayer.flushUpdate guarding
      // against `hasLiveStyle === false` so it bails harmlessly after
      // map.remove() runs.
      // Still release the module-level VT registry (the flat geometry arrays
      // MapLibre doesn't own) - that piece survives map.remove() and is keyed
      // by a fixed source id shared across projects, so skipping it leaks this
      // project's arrays until the next project publishes (#mem). Cheap: JS-map
      // clears only, no map/worker work, so it doesn't reintroduce the freeze.
      surveyLayerRef.current?.releaseVtRegistry();
      surveyLayerRef.current = null;
      const tA = performance.now();
      map.current?.remove();
      const tB = performance.now();
      map.current = null;
      debugLog(`[perf:mapcanvas-unmount] preRemove=${(tA - t0).toFixed(0)}ms  mapRemove=${(tB - tA).toFixed(0)}ms`);
      void import('../../utils/memoryProbe').then(({ recordPerfTiming }) => {
        recordPerfTiming('map.remove()', tB - tA);
      });
    };
  }, []);

  // Scroll-zoom speed: map.controls.zoom_speed is zoom LEVELS per 100 px wheel
  // notch, and utils/zoomRate.ts inverts MapLibre's sigmoid to the rate that
  // delivers them. Handing the setting over as speed/100 instead made the top
  // of its range inert - the sigmoid is asymptotic at one level per notch, so
  // 3.0 and 5.0 were 0.930 and 0.990 levels, a difference nobody can feel
  // (#429). Shift+wheel still applies MapLibre's 1/4-rate precision modifier.
  //
  // Both MapLibre rates (trackpad AND wheel) are set together, on purpose:
  // MapLibre's per-chain wheel-vs-trackpad classifier misfires on Windows
  // precision trackpads (measured ~70% wheel-classified events on gestures that
  // felt sluggish, and the verdict can flap mid-gesture), and with equal rates
  // its verdict has no visible effect.
  const zoomLevelsPerNotch = config?.map.controls.zoom_speed ?? ZOOM_LEVELS_PER_NOTCH_DEFAULT;
  useEffect(() => {
    if (!mapLoaded || !map.current) return;
    const rate = wheelZoomRateForLevels(zoomLevelsPerNotch);
    map.current.scrollZoom.setZoomRate(rate);
    map.current.scrollZoom.setWheelZoomRate(rate);
  }, [mapLoaded, zoomLevelsPerNotch]);

  // Invert scroll: MapLibre has no public wheel-direction option, but its
  // HandlerManager looks up `scrollZoom.wheel` at dispatch time, so replacing
  // the method on the instance is a stable interception point. The proxy flips
  // deltaY only; every other read (clientX/Y for the zoom anchor, deltaMode,
  // preventDefault, the retained _lastWheelEvent) forwards to the original
  // event, with methods bound so invocation stays legal.
  const invertScroll = config?.map.controls.invert_scroll ?? false;
  useEffect(() => {
    if (!mapLoaded || !map.current || !invertScroll) return;
    const sz = map.current.scrollZoom as unknown as { wheel: (e: WheelEvent) => void };
    const orig = sz.wheel.bind(map.current.scrollZoom);
    sz.wheel = (e: WheelEvent) =>
      orig(
        new Proxy(e, {
          get(target, prop) {
            if (prop === 'deltaY') return -target.deltaY;
            const v = Reflect.get(target, prop);
            return typeof v === 'function' ? v.bind(target) : v;
          },
        }) as WheelEvent
      );
    return () => {
      sz.wheel = orig;
    };
  }, [mapLoaded, invertScroll]);

  // Register the custom tile protocol; the backend caches responses under the
  // provider's own Cache-Control (see src-tauri/src/cache/tiles.rs)
  // Uses ref-counted registration to safely handle multiple map instances
  useEffect(() => {
    registerTileProtocol();
    return () => {
      unregisterTileProtocol();
    };
  }, []);

  // Expose map instance for E2E testing and keyboard shortcuts
  useEffect(() => {
    if (map.current) {
      (window as unknown as { __stalagmight_map__?: maplibregl.Map }).__stalagmight_map__ = map.current;
    }
    return () => {
      delete (window as unknown as { __stalagmight_map__?: maplibregl.Map }).__stalagmight_map__;
    };
  }, [mapLoaded]);

  // Map E2E hooks (drawn geometry, measured-path tint, drag-ghost offset, the
  // active meters transform). The custom renderer makes MapLibre's feature/paint
  // queries useless to tests, so these expose what is actually drawn - see
  // src/dev/mapE2eHooks.ts for the full rationale.
  useEffect(
    () =>
      installMapE2eHooks({
        surveyLayer: () => surveyLayerRef.current,
        toMeters: (lng, lat) => mapCoordsToMeters(lng, lat, gpsOrigin),
        ...(import.meta.env.DEV || import.meta.env.MODE === 'test' || import.meta.env.VITE_E2E === '1'
          ? {
              controls: {
                interactionSequence: () => mapInteractionSequenceRef.current,
                setSemanticTarget: (target) => {
                  const layer = surveyLayerRef.current;
                  if (!layer) return false;
                  layer.setSemanticTargetPreference(target);
                  return true;
                },
                clearSemanticTarget: () => surveyLayerRef.current?.setSemanticTargetPreference(null),
              },
            }
          : {}),
      }),
    [surveyLayerRef, mapLoaded, gpsOrigin],
  );

  // Update survey layer when data changes or map loads.
  // useLayoutEffect ensures setData() runs before the browser paints - critical
  // for drag commit where endDragPreview() must fire in the same frame as setData().
  useLayoutEffect(() => {
    if (surveyLayerRef.current && project) {
      surveyLayerRef.current.update(
        project.sections,
        activeStations,
        // Closure overlays are drawn from the active coordinates, so an adjusted
        // (closed) loop collapses to a zero-length, invisible line, while a loop
        // that wasn't closed - an ineligible/deferred closure, or a component whose
        // solve fell back to measured - stays visible. Pass closures in both modes
        // rather than hiding them wholesale, so the adjusted view doesn't look
        // resolved when some loops are still open.
        project.loop_closures,
        gpsOrigin,
        effectiveDepthGradient,
        depthRange,
        precision,
        issueInput
      );
    }
  }, [project, activeStations, mapLoaded, gpsOrigin, effectiveDepthGradient, depthRange, precision, geometryMode, surveyDataAdjusted, issueInput]);

  // (The Warnings-tab auto-highlight is now derived via `effectiveIssueSeverity`
  // above - no snapshot/restore effect needed, so nothing to lose on leave/unmount.)

  // On a project switch (created_at changes on switch, stable across edits AND
  // saves), reset the distance origin to the entrance default and drop the stale
  // field - otherwise the previous project's distances paint the new project gray
  // (#34). Keyed on created_at alone so Save As (a file_path change) does NOT move
  // the origin.
  useEffect(() => {
    setDistanceGradientOrigin(null);
  }, [project?.created_at, setDistanceGradientOrigin]);

  // Same project-switch reset for the gas origin (#38): drop the picked origin (and
  // its cached field) so the new project's reachable overlay re-seeds from its
  // entrance. Keyed on created_at alone, like the distance reset.
  useEffect(() => {
    useGasPlanStore.getState().setOrigin(null);
  }, [project?.created_at]);

  // Reset the ruler on a switch too, but keyed on file_path + created_at so opening
  // a distinct file that happens to share created_at - e.g. a duplicated project -
  // still clears stale ruler state (#26, Codex). (Save As also clears the ruler,
  // which is harmless; the modified_at effect below covers same-file edits.)
  const projectSwitchKey = `${project?.file_path ?? ''}|${project?.created_at ?? ''}`;
  useEffect(() => {
    clearMeasure();
  }, [projectSwitchKey, clearMeasure]);

  // Also reset the ruler when the SAME project's geometry/data changes (undo/redo
  // or any edit bumps modified_at): a persisted readout + path tint computed from
  // the old geometry would otherwise survive a setData() rebuild and silently lie.
  // Kept separate from the switch reset above so an edit doesn't also move the
  // distance-gradient origin (intentionally stable across edits) (#26, Codex).
  //
  // `gas.include_reference_legs` counts as such a change (#27): the along-line
  // distance is computed with reference ties either traversable or not, so
  // flipping the setting invalidates a live readout exactly the way an edit does
  // - and it does NOT bump modified_at, so it needs its own dep.
  useEffect(() => {
    clearMeasure();
  }, [project?.modified_at, config?.gas.include_reference_legs, clearMeasure]);

  // In closureAdjusted mode the map first renders the measured fallback, then swaps
  // in the adjusted geometry when it finishes loading (async, with no modified_at
  // change). A measurement captured against the fallback would then describe the old
  // geometry, so clear it when the active adjusted geometry changes. Gated on the
  // adjusted mode so a background adjusted (in)validation can't clear a measurement
  // taken in the measured view (#26, Codex).
  useEffect(() => {
    if (geometryMode === 'closureAdjusted') clearMeasure();
  }, [stationsAdjusted, surveyDataAdjusted, geometryMode, clearMeasure]);

  // Distance-gradient overlay (#34): fetch the per-station distance field when the
  // overlay is on and the origin or project data changes. Distances use MEASURED
  // geometry (surveyed lengths), so neither closure-adjustment config nor the
  // display geometryMode affects them - no need to refetch on either. `from` null
  // = the entrance (zero-datum) default.
  //
  // `gas.include_reference_legs` IS an input, though (#27): the backend reads it
  // per call to decide whether a reference tie is traversable, so flipping it in
  // Settings changes the whole field. It has to be in the deps or the ramp keeps
  // showing the pre-toggle routing until some unrelated edit bumps modified_at.
  useEffect(() => {
    if (!showDistanceGradient) return undefined;
    let cancelled = false;
    invokeIpc<DistanceField>(
      'get_station_distances_from',
      { from: distanceGradientOrigin ?? null },
      { purpose: 'distance gradient field', fields: { origin: distanceGradientOrigin ?? 'entrance' } }
    )
      .then((field) => { if (!cancelled) { setDistanceField(field); setDistanceFieldFailed(false); } })
      .catch((e) => {
        console.warn('[distance-gradient] field fetch failed', e);
        // Clear on failure (#34): drop any stale field + flag the failure so the
        // apply effect restores base colors instead of holding the old ramp
        // against the new origin/geometry. Guard on the cancel token so a
        // superseded fetch never clears a newer in-flight one.
        if (!cancelled) { setDistanceField(null); setDistanceFieldFailed(true); }
      });
    return () => { cancelled = true; };
  }, [
    showDistanceGradient,
    distanceGradientOrigin,
    project?.modified_at,
    config?.gas.include_reference_legs,
    setDistanceField,
  ]);

  // Gas reachable-frontier overlay (#38): fetch the reachability field when the
  // sub-mode is on and any input (origin / diver / cylinders / config) or the
  // project data changes. Like distances it uses MEASURED geometry, so the display
  // geometryMode / closure config don't affect it. No debounce: the compute is
  // ms-fast and the inputs are discrete (number fields / presets), not continuous
  // sliders. `origin` null = the entrance (zero-datum) default; the command
  // returns null when no project is open (Ok(None)) or throws on invalid input.
  useEffect(() => {
    if (!showGasPlan) return undefined;
    // R9/R10 (#395): before the diver has answered "Where does this dive start?" there is
    // nothing to shade - no shading, no markers, no legend (the legend is already gated on
    // `gasReachField`). Clear any stale field left from a previous session/project so the entry
    // card never resurrects a leftover ramp.
    if (!gasOriginChosen) {
      // Not merely "field not yet fetched" - flag it like a failed fetch, or
      // `overlayApplyAction` reads the null field as "recompute in flight" and
      // HOLDS the previous project's ramp under the entry card (#395). The next
      // origin choice + successful fetch clears the flag.
      setGasReachField(null);
      setGasFieldFailed(true);
      return undefined;
    }
    if (gasComputeGate.skip) {
      // A stalled plan (#310) runs NO compute at all, and the store has already nulled the
      // field - flag it like a failed fetch, or `overlayApplyAction` reads the null field as
      // "recompute in flight" and holds the last ramp's reachable colors under the
      // unresolved-pin warning indefinitely. The next successful fetch clears the flag.
      setGasFieldFailed(true);
      return undefined;
    }
    let cancelled = false;
    invokeIpc<GasReachField | null>(
      'compute_gas_reach_field',
      {
        origin: gasComputeGate.origin,
        endpoint: gasComputeGate.endpoint,
        // Ordered way points (#298): the engine re-bases the whole field through them, so this is
        // a gas input, not a drawing hint.
        waypoints: gasWaypoints,
        // Per-span pace/propulsion (#317): also a gas input - the engine derates in-leg
        // consumption while a span is `on_scooter`.
        segments: gasSegments,
        diver: gasDiver,
        cylinders: gasCylinders,
        gasConfig: gasPlanConfig,
        // Per-stage drop overrides (#246): the station each ▼ marker was dragged to. Empty when
        // nothing is dragged; the backend clamps each pin before it can move the budget.
        stageDropOverrides: gasComputeGate.stageDropOverrides,
      },
      { purpose: 'gas reachable-frontier field', fields: { origin: gasComputeGate.origin ?? 'entrance' } }
    )
      .then((field) => { if (!cancelled) { setGasReachField(field); setGasFieldFailed(false); } })
      .catch((e) => {
        console.warn('[gas-plan] field fetch failed', e);
        // Clear on failure (#38): mirror the distance overlay - drop the stale
        // field + flag the failure so the apply effect restores base colors
        // rather than holding the old ramp against the new gas inputs.
        if (!cancelled) { setGasReachField(null); setGasFieldFailed(true); }
      });
    return () => { cancelled = true; };
  }, [showGasPlan, gasOriginChosen, gasComputeGate, gasWaypoints, gasSegments, gasDiver, gasCylinders, gasPlanConfig, project?.modified_at, setGasReachField]);

  // The user's depth / distance palette + Levels range (legend cog), live while
  // a palette is previewed or a Levels handle dragged. Store-subscribed, so a drag
  // never re-renders MapCanvas.
  useSurveyLayerColorScales(surveyLayerRef, mapLoaded);

  // Apply the fetched field to the layer (feature-state + paint swap). Separate
  // from the rebuild path so re-rooting never triggers a feature rebuild.
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    if (showDistanceGradient && !effectiveIssueSeverity && distanceField && surveyData) {
      // Measured SoA: distances use surveyed geometry, and the builder only needs
      // section metadata + shot ids - never the lazy `section.shots` objects.
      const state = buildDistanceFeatureState(surveyData, distanceField);
      surveyLayerRef.current.setDistanceGradient(true, state);
    } else if (overlayApplyAction(showDistanceGradient && !effectiveIssueSeverity, distanceField != null, surveyData != null, distanceFieldFailed) === 'off') {
      // Real off (mode disabled, severity coloring wins, no geometry, or a FAILED
      // recompute) -> base now. NOT while the origin is recomputing successfully
      // (field transiently null, mode still on, no failure): 'hold' keeps the old
      // ramp so a re-root never blinks base (#customRender).
      surveyLayerRef.current.setDistanceGradient(false, null);
    }
  }, [showDistanceGradient, effectiveIssueSeverity, distanceField, surveyData, mapLoaded, surveyLayerRef, distanceFieldFailed]);

  // Apply the gas field to the layer (feature-state + paint swap). Separate from
  // the fetch so re-rooting / re-computing never triggers a feature rebuild.
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    if (showGasPlan && gasReachField && surveyData) {
      // Measured SoA (see distance-gradient effect): builder reads shot ids from
      // typed arrays, never the lazy `section.shots` objects.
      const state = buildGasReachFeatureState(surveyData, gasReachField);
      surveyLayerRef.current.setGasReach(true, state);
    } else if (overlayApplyAction(showGasPlan, gasReachField != null, surveyData != null, gasFieldFailed) === 'off') {
      // 'hold' during a SUCCESSFUL gas param/origin recompute (field transiently
      // null) keeps the old ramp; a real off OR a failed recompute restores base
      // (mirrors the distance guard).
      surveyLayerRef.current.setGasReach(false, null);
    }
  }, [showGasPlan, gasReachField, surveyData, mapLoaded, surveyLayerRef, gasFieldFailed]);

  // Plan-mode idle tint (#395 revision): while Plan is active but no gas origin has
  // been chosen yet, tint the whole survey the gas ramp's dim far color instead of
  // leaking section colors under the gas color scheme. Driven purely off
  // `gasOriginChosen`, not off `gasReachField` - SurveyLayer itself yields to a real
  // field the moment one is set (see setGasReach / isPlanIdleActive), so this clears
  // the instant an origin is chosen even before the field's first fetch resolves.
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    surveyLayerRef.current.setPlanIdleTint(showGasPlan && !gasOriginChosen);
  }, [showGasPlan, gasOriginChosen, mapLoaded, surveyLayerRef]);

  // Highlight the picked route over the survey (#233): full-opacity swum legs + faded jumps (#243).
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    if (showGasPlan && gasReachField && project) {
      surveyLayerRef.current.setGasRoute(
        gasReachField.route,
        routeLegSegmentIndices(gasReachField).map(segmentColor),
      );
    } else {
      surveyLayerRef.current.setGasRoute([]);
    }
  }, [showGasPlan, gasReachField, project, mapLoaded, surveyLayerRef]);

  // Tint the measured along-line path's shots AND stations (#26) in the
  // user-configured color. Keyed on the along-line result so it applies when the
  // IPC resolves and clears on reset (clearMeasure nulls measureAlongLine).
  const measurePathColor = config?.display.measure_path_color ?? MEASURE_TINT;
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    // Only tint while a completed measurement exists. Gating on measureEnd (not
    // measureAlongLine alone) means a late along-line response that lands after a
    // reset can't repaint a stale path with no active measurement (#26).
    const alongLine = measureEnd ? measureAlongLine : null;
    const shotIds = alongLine ? buildMeasuredPathIds(alongLine.pathShots) : null;
    const stationIds = alongLine ? buildMeasuredStationIds(alongLine.pathStations) : null;
    surveyLayerRef.current.setMeasuredPath(
      shotIds && shotIds.size > 0 ? shotIds : null,
      stationIds && stationIds.size > 0 ? stationIds : null,
      measurePathColor,
    );
  }, [measureAlongLine, measureEnd, mapLoaded, measurePathColor]);

  // Distance-gradient origin marker (#34): a ring on the chosen origin station.
  // A MapLibre Marker so it auto-tracks the camera and shows even when the
  // stations layer is hidden (common with the gradient on). Only for a *picked*
  // origin - the entrance default is the zero datum(s), already starred.
  useEffect(() => {
    originMarkerRef.current?.remove();
    originMarkerRef.current = null;
    // Suppressed while issue-severity owns the paint (the distance overlay isn't
    // drawn then), so the picked-origin ring doesn't linger over a severity map.
    if (!map.current || !mapLoaded || !showDistanceGradient || effectiveIssueSeverity || !distanceGradientOrigin) {
      return undefined;
    }
    const originStation = activeStations.find(
      (s) =>
        s.station_ref.section_id === distanceGradientOrigin.section_id &&
        s.station_ref.station_index === distanceGradientOrigin.station_index
    );
    if (!originStation) return undefined;
    const lngLat = metersToMapCoords(
      originStation.coordinates.x,
      originStation.coordinates.y,
      gpsOrigin
    ) as [number, number];
    const el = document.createElement('div');
    el.setAttribute('data-testid', 'distance-origin-marker');
    el.style.cssText =
      'width:18px;height:18px;border-radius:50%;border:3px solid #7d3c98;' +
      'box-shadow:0 0 0 2px #ffffff,0 0 4px rgba(0,0,0,0.6);' +
      'background:rgba(255,255,255,0.15);pointer-events:none;';
    originMarkerRef.current = new maplibregl.Marker({ element: el })
      .setLngLat(lngLat)
      .addTo(map.current);
    return () => {
      originMarkerRef.current?.remove();
      originMarkerRef.current = null;
    };
  }, [showDistanceGradient, effectiveIssueSeverity, distanceGradientOrigin, activeStations, gpsOrigin, mapLoaded]);

  // Gas-plan origin marker (#38): a teal ring on the chosen origin (mirrors the
  // distance origin marker; teal to distinguish "start" from the gold→purple
  // reachable ramp). Only for a *picked* origin - the entrance default is the
  // zero datum(s), already starred.
  useEffect(() => {
    gasOriginMarkerRef.current?.remove();
    gasOriginMarkerRef.current = null;
    if (!map.current || !mapLoaded || !showGasPlan || !gasOrigin) return undefined;
    const originStation = activeStations.find(
      (s) =>
        s.station_ref.section_id === gasOrigin.section_id &&
        s.station_ref.station_index === gasOrigin.station_index
    );
    if (!originStation) return undefined;
    const lngLat = metersToMapCoords(
      originStation.coordinates.x,
      originStation.coordinates.y,
      gpsOrigin
    ) as [number, number];
    const el = document.createElement('div');
    el.setAttribute('data-testid', 'gas-origin-marker');
    el.style.cssText =
      'width:18px;height:18px;border-radius:50%;border:3px solid #16a085;' +
      'box-shadow:0 0 0 2px #ffffff,0 0 4px rgba(0,0,0,0.6);' +
      'background:rgba(255,255,255,0.15);pointer-events:none;';
    gasOriginMarkerRef.current = new maplibregl.Marker({ element: el })
      .setLngLat(lngLat)
      .addTo(map.current);
    return () => {
      gasOriginMarkerRef.current?.remove();
      gasOriginMarkerRef.current = null;
    };
  }, [showGasPlan, gasOrigin, activeStations, gpsOrigin, mapLoaded]);

  // Plan point markers (#38 datum stars, #298 way points + marked points) - simple place/click
  // pins, extracted to their own hook. The turn-frontier dots and stage drops stay here because
  // they carry drag + focus-restore machinery tied to this component's refs.
  useGasPlanPointMarkers({
    map: map.current,
    mapLoaded,
    showGasPlan,
    activeStations,
    gpsOrigin,
    waypoints: gasWaypoints,
    markedPoints: gasMarkedPoints,
    setOrigin: setGasOrigin,
  });

  // Staged-strategy ▼ drop markers + the final-turn marker (#233/#246) - draggable/nudgeable route
  // pins, extracted to their own hook. See useGasPlanDropMarkers.ts for why they don't share the
  // simpler point-marker hook above.
  useGasPlanDropMarkers({
    map: map.current,
    mapLoaded,
    showGasPlan,
    gasReachField,
    activeStations,
    gpsOrigin,
    setStageDropOverride: setGasStageDropOverride,
  });

  // Lazily fetch closure-adjusted geometry when the adjusted view is active and
  // we don't have it yet (first switch, or after an edit invalidated it). The
  // store action guards against concurrent/duplicate fetches. `adjustedLoading`
  // is a dependency so that when an in-flight solve is discarded because the
  // project changed (it clears the flag without storing data), this effect
  // re-runs and fetches the new project's geometry instead of staying on the
  // measured fallback.
  useEffect(() => {
    if (geometryMode === 'closureAdjusted' && project && !surveyDataAdjusted && !adjustedLoading) {
      void loadAdjustedGeometry().then((ok) => {
        // On a genuine fetch failure, fall back to the measured view rather than
        // leaving the user stuck in a read-only view showing measured geometry.
        if (!ok) {
          setGeometryMode('measured');
          showToast(t('map.geometry.computeFailed'), TOAST_KEYS.GEOMETRY_COMPUTE_FAILED, 'error');
        }
      });
    }
  }, [geometryMode, project, surveyDataAdjusted, adjustedLoading, loadAdjustedGeometry, setGeometryMode, showToast, t]);

  // The adjusted view is meaningless without an eligible closure: revert to
  // measured if the last adjustable closure is removed while it is active.
  useEffect(() => {
    if (geometryMode === 'closureAdjusted' && project && !hasEligibleClosures(project.loop_closures)) {
      setGeometryMode('measured');
    }
  }, [geometryMode, project, setGeometryMode]);

  // The adjusted view is read-only - and so is the whole map in Plan/Export, even on a
  // project with no closures where the display falls back to measured geometry. Drop any
  // geometry-mutating tool back to select in either case (appMode is the robust authority;
  // geometryMode only drives which coords are drawn).
  // A remote SpeleoDB project opened without the write lock is read-only too, so
  // drop a mutating tool back to select on open (mirrors closure-adjusted).
  useEffect(() => {
    if ((isMapReadOnly({ geometryMode, appMode }) || speleoReadOnly) && isMutatingTool(activeTool)) {
      setActiveTool('select');
    }
  }, [geometryMode, appMode, speleoReadOnly, activeTool, setActiveTool]);

  // Turning the navigate group off (#409) has to take Pan/Zoom off the MAP too, not
  // just out of the toolbar: the tool is persisted UI state, so a user who was on Pan
  // when they flipped the setting would otherwise be stuck with a tool that has no
  // button to leave it with. The predicate is pure and lives in toolUtils so its
  // null-config boot semantics are pinned by unit tests (kicking a restored Pan/Zoom
  // tool before the real config arrives is the regression they guard).
  useEffect(() => {
    if (shouldResetNavigationTool(config, activeTool)) {
      setActiveTool('select');
    }
  }, [config, activeTool, setActiveTool]);

  // Update callbacks when they change. `mapLoaded` is a dependency on purpose:
  // SurveyLayer is constructed in the async map 'load' handler with whatever
  // `toolCallbacks` the map-creation effect closed over on first render, and
  // `toolCallbacks` is memoized (#430), so it can settle to a stable identity
  // while `surveyLayerRef.current` is still null and this effect no-ops. Re-run
  // it on the setMapLoaded(true) render to apply the current callbacks.
  useEffect(() => {
    if (surveyLayerRef.current) {
      surveyLayerRef.current.setCallbacks(toolCallbacks);
    }
  }, [toolCallbacks, mapLoaded]);

  // Sync config with SurveyLayer for visual and hitbox layer updates
  useEffect(() => {
    if (surveyLayerRef.current && config) {
      surveyLayerRef.current.setConfig(config);
    }
  }, [config, mapLoaded]);

  // Apply theme colors to SurveyLayer and map background when resolvedTheme changes
  // MapLibre GL JS does NOT read CSS variables - colors must be re-applied explicitly
  useEffect(() => {
    const colors = getMapThemeColors();
    if (surveyLayerRef.current) {
      surveyLayerRef.current.applyThemeColors(colors);
    }
    // Re-apply map background color to match the active theme
    if (map.current && mapLoaded) {
      map.current.setPaintProperty('background', 'background-color', colors.background);
    }
  }, [resolvedTheme, mapLoaded]);

  // Sync selection state with SurveyLayer for visual feedback.
  // useLayoutEffect (not useEffect) so it clears before paint, in the same commit
  // as the geometry-apply layout effect above. On a project switch the geometry
  // rebuild runs first and re-applies the controller's stored highlight; because
  // section ids are per-project sequential (they collide across projects), a
  // regular useEffect would let the colliding section flash highlighted for one
  // frame before this cleared it.
  //
  // Each branch sets the new target BEFORE clearing the other two: every setter
  // re-raises the selection's section (`raisedSectionOf`, section > station >
  // shot), and this order moves the draw order at most once per change instead
  // of rewriting it for an intermediate empty selection.
  useLayoutEffect(() => {
    if (!surveyLayerRef.current) return;

    if (selection?.type === 'station' && selection.stationRef) {
      surveyLayerRef.current.setSelectedStation(selection.stationRef);
      surveyLayerRef.current.setSelectedShot(null, null);
      surveyLayerRef.current.setSelectedSection(null);
    } else if (selection?.type === 'shot' && selection.sectionId !== undefined && selection.shotId !== undefined) {
      surveyLayerRef.current.setSelectedShot(selection.sectionId, selection.shotId);
      surveyLayerRef.current.setSelectedStation(null);
      surveyLayerRef.current.setSelectedSection(null);
    } else if (selection?.type === 'section' && selection.sectionId !== undefined) {
      // Section selection
      surveyLayerRef.current.setSelectedSection(selection.sectionId);
      surveyLayerRef.current.setSelectedStation(null);
      surveyLayerRef.current.setSelectedShot(null, null);
    } else {
      surveyLayerRef.current.setSelectedStation(null);
      surveyLayerRef.current.setSelectedShot(null, null);
      surveyLayerRef.current.setSelectedSection(null);
    }
  }, [selection]);

  // Sync snapped station with SurveyLayer for snap indicator
  // (Also called directly from checkSnapToStation for immediate visual response;
  // this effect is the fallback for store-driven changes like tool-change clears.)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setSnappedStation(snappedStation);
  }, [snappedStation]);

  // Hover sync effects moved to <MapHoverSync> child component to avoid
  // re-rendering this large component on every mousemove (~60fps).

  // Sync highlight layer with snap/merge state
  // Shows snapped/origin stations even when stations layer is hidden
  useEffect(() => {
    if (!surveyLayerRef.current) return;

    const highlightIds: string[] = [];

    const snappedId = stationRefToId(snappedStation);
    if (snappedId) highlightIds.push(snappedId);

    const mergeAId = stationRefToId(mergeStationA);
    if (mergeAId && !highlightIds.includes(mergeAId)) highlightIds.push(mergeAId);

    const mergeBId = stationRefToId(mergeStationB);
    if (mergeBId && !highlightIds.includes(mergeBId)) highlightIds.push(mergeBId);

    surveyLayerRef.current.setHighlightedStations(highlightIds);
  }, [snappedStation, mergeStationA, mergeStationB, mapLoaded]);

  // Sync shot data labels visibility with SurveyLayer (gated by showShots)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowShotLabels(getEffectiveShowShotData(showShots, showShotData));
  }, [showShotData, showShots, mapLoaded]);

  // Sync section name labels visibility with SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowSectionNames(showSectionNames);
  }, [showSectionNames, mapLoaded]);

  // Sync the station labels (off / IDs / names) with SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setStationLabelMode(getEffectiveStationLabelMode(showStationLabels, showStationNames));
  }, [showStationLabels, showStationNames, mapLoaded]);

  // Sync station comments visibility with SurveyLayer (gated by showStations)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowStationComments(getEffectiveShowStationComments(showStations, showStationComments));
  }, [showStationComments, showStations, mapLoaded]);

  // Sync shot comments visibility with SurveyLayer (gated by showShots)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowShotComments(getEffectiveShowShotComments(showShots, showShotComments));
  }, [showShotComments, showShots, mapLoaded]);

  // Hide source comment on map during move_comment pickup (no backend mutation on pickup)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    if (moveCommentSource) {
      if (moveCommentSource.type === 'station' && moveCommentSource.stationRef) {
        surveyLayerRef.current.setHiddenComment({
          type: 'station',
          sectionId: moveCommentSource.stationRef.section_id,
          id: moveCommentSource.stationRef.station_index,
        });
      } else if (moveCommentSource.type === 'shot' && moveCommentSource.sectionId !== undefined && moveCommentSource.shotId !== undefined) {
        surveyLayerRef.current.setHiddenComment({
          type: 'shot',
          sectionId: moveCommentSource.sectionId,
          id: moveCommentSource.shotId,
        });
      }
    } else {
      surveyLayerRef.current.setHiddenComment(null);
    }
  }, [moveCommentSource, mapLoaded]);

  // Sync stations visibility with SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowStations(showStations);
  }, [showStations, mapLoaded]);

  // Sync shots visibility with SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowShots(showShots);
  }, [showShots, mapLoaded]);

  // Sync reference shots visibility with SurveyLayer (gated by showShots)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowReferenceShots(getEffectiveShowReferenceShots(showShots, showReferenceShots));
  }, [showReferenceShots, showShots, mapLoaded]);

  // Sync jump shots visibility (#27): jumps hide independently of references.
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowJumps(getEffectiveShowJumps(showShots, showJumps));
  }, [showJumps, showShots, mapLoaded]);

  // Sync anchored section IDs with SurveyLayer for fading.
  // useLayoutEffect (not useEffect) for the same reason as the selection sync
  // above: the controller retains its dim state across a project switch, and
  // section/feature ids collide across projects. A post-paint useEffect would
  // let the pre-paint geometry rebuild re-apply the previous project's dim to
  // the new project's colliding ids for one frame before this cleared it.
  useLayoutEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setAnchoredSections(anchoredSectionIds);
  }, [anchoredSectionIds, mapLoaded]);

  // Sync immovable feature IDs with SurveyLayer for disabled visual.
  // useLayoutEffect for the same cross-project-flash reason as above; immovable
  // ids are non-empty for the entire duration of a move tool, so this is the
  // more reachable trigger of the stale-dim flash.
  useLayoutEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setImmovableFeatures(
      immovableFeatureIds.stationIds,
      immovableFeatureIds.shotIds
    );
  }, [immovableFeatureIds, mapLoaded]);

  // Clear station selection, hover, and cancel merge when stations hidden
  // Note: Draw/Measure/Snap operations continue to work - highlight layer shows snapped stations
  useEffect(() => {
    if (!showStations) {
      // Clear station selection and hover (these require visible stations to interact)
      clearSelectionByType('station');
      setHoveredStation(null);
      surveyLayerRef.current?.clearStationHover();
      // Cancel merge (requires visible stations to select targets)
      clearMergeStations();
      // Cancel placement mode properly (calls onCancel callback and clears cursor)
      if (placementMode) {
        handlePlacementCancel();
      }
      // Note: Do NOT clear snappedStation or measure state
      // These operations work with hidden stations - highlight layer shows them
    }
  }, [showStations, clearSelectionByType, setHoveredStation, clearMergeStations, placementMode, handlePlacementCancel]);

  // Clear shot selection and hover when shots visibility changes to OFF
  useEffect(() => {
    if (!showShots) {
      clearSelectionByType('shot');
      setHoveredShot(null);
      surveyLayerRef.current?.clearShotHover();
    }
  }, [showShots, clearSelectionByType, setHoveredShot]);

  // Drop the selection of any shot a kind toggle just hid. Jumps and References
  // are independent (#27), so this has to run for either going OFF and has to
  // judge each selected shot by its own kind - keying the whole effect off the
  // reference toggle deselected still-visible jumps.
  // Keyed on `project?.sections`, not a project-identity string: a shot's kind
  // is editable (context menu, the J/R table cell, a fill-drag), and those go
  // through executeCommand, which bumps `modified_at` but never `file_path` /
  // `created_at`. Keying on identity meant marking the SELECTED shot as a Jump
  // while Jumps were hidden left it selected and in the Detail panel after it
  // vanished from the map. The measure-snap effect above already keys this way.
  useEffect(() => {
    if ((showReferenceShots && showJumps) || !project?.sections) return;
    filterSelection((s) => {
      if (s.type !== 'shot') return true;
      const section = project.sections.find((sec) => sec.id === s.sectionId);
      const shot = section?.shots.find((sh) => sh.id === s.shotId);
      return isShotKindVisible(shot?.kind, showShots, showJumps, showReferenceShots);
    });
    surveyLayerRef.current?.clearShotHover();
    setHoveredShot(null);
  }, [showShots, showJumps, showReferenceShots, project?.sections, filterSelection, setHoveredShot]);

  // The scale bar is always shown. Add the control once the map is loaded.
  useEffect(() => {
    if (!map.current || !scaleControlRef.current || !mapLoaded) return;
    if (scaleControlAddedRef.current) return;

    try {
      map.current.addControl(scaleControlRef.current, 'bottom-left');
      scaleControlAddedRef.current = true;
    } catch (e) {
      // Control may not be in a state to be added - ignore errors
      console.warn('[MapCanvas] Scale control add error:', e);
    }
  }, [mapLoaded]);

  // Update cursor based on tool (and Ctrl state for select mode)
  useEffect(() => {
    if (!map.current) return;

    // Placement mode manages its own cursor - don't override
    if (placementMode) return;

    // In supported tools with Ctrl held, show grab cursor for panning
    const ctrlPan = activeTool !== 'pan' && ctrlHeld;
    const base = ctrlPan ? 'grab' : toolCursor(activeTool);

    // If hovering over a feature, SurveyLayer sets cursor to 'pointer'
    // Otherwise, set cursor based on active tool
    // Force zoom-in cursor for zoom tool even when hovering over features
    if (!isHoveringFeature || activeTool === 'zoom') {
      map.current.getCanvas().style.cursor = base;
    }
    // Hand the same value to SurveyLayer, which writes this property too and needs
    // something to fall back to when nothing is hovered. It cannot derive `base`
    // itself - the tool map and the Ctrl-held grab both live here - and without it
    // its own recomputes (which continue after this passive effect has run for the
    // last time) reset the cursor to a bare arrow (#24). Unconditional: it is the
    // RESTING cursor, true whether or not something is hovered right now.
    surveyLayerRef.current?.setBaseCursor(base, ctrlPan);
    // `mapLoaded` is a dependency for the hand-off above, not for the write: on the
    // first run the layer does not exist yet, and none of the other deps change on
    // map load, so without it the starting tool's resting cursor would never reach
    // SurveyLayer at all.
  }, [activeTool, ctrlHeld, isHoveringFeature, placementMode, mapLoaded]);

  // Narrowed to a boolean before the gesture effects consume it: `placementMode`
  // is replaced wholesale on every cursor update, so depending on the object
  // itself would re-run them on each pointer move during placement.
  const placementActive = placementMode !== null;

  // ONE gesture decision for the whole component, destructured to primitives so
  // the effects below can depend on it. Three consumers read it: the dragPan
  // effect right here, the custom modifier-pan (which claims a Ctrl+left press
  // outright only where MapLibre's dragPan is OFF), and the container's
  // touch-action. Sharing the decision rather than re-deriving it from the same
  // inputs is what stops them disagreeing about who owns the drag. An in-flight
  // move ghost is deliberately NOT an input: carrying one leaves the drag to
  // MapLibre and click-gates the drop instead (see `handlePointerUp`).
  const { dragPan: mapLibreDragPan, touchAction } = mapGestureMode({
    activeTool,
    placementActive,
  });

  // Hand plain left-drag to MapLibre unless the current tool or mode owns it.
  useEffect(() => {
    if (!map.current) return;

    if (mapLibreDragPan) {
      map.current.dragPan.enable();
    } else {
      map.current.dragPan.disable();
    }
  }, [mapLibreDragPan]);

  // Toggle doubleClickZoom based on active tool (disable for zoom tool, enable otherwise)
  useEffect(() => {
    if (!map.current) return;
    if (activeTool === 'zoom') {
      map.current.doubleClickZoom.disable();
    } else {
      map.current.doubleClickZoom.enable();
    }
  }, [activeTool]);

  // Fetch anchored section IDs when Move Segment tool becomes active.
  // Sets anchoredIdsLoaded=false immediately on activation, then true once fetched,
  // so drag start is blocked until the anchored set is populated.
  // Uses a cancelled flag to ignore stale results from late promise resolution
  // when activeTool or project changes before the fetch completes.
  useEffect(() => {
    let cancelled = false;
    if (activeTool === 'move_segment') {
      clearSelectionAndVisuals();
      setAnchoredIdsLoaded(false);
      getAnchoredSectionIds().then((ids) => {
        if (cancelled) return;
        setAnchoredSectionIds(new Set(ids));
        setAnchoredIdsLoaded(true);
      }).catch(() => {
        if (cancelled) return;
        // Fail closed: keep anchoredIdsLoaded=false so segment drag remains blocked.
        // This prevents moving anchored segments when the IPC fails.
        setAnchoredSectionIds(new Set());
        showToast(t('map.toasts.anchoredSegmentsError'), TOAST_KEYS.ANCHORED_IDS_ERROR, 'error');
      });
    } else {
      setAnchoredSectionIds(new Set());
      setAnchoredIdsLoaded(false);
    }
    return () => { cancelled = true; };
  }, [activeTool, clearSelectionAndVisuals, getAnchoredSectionIds, project, showToast]);

  // Wire up setActiveTool - depend on mapLoaded to ensure it runs after layer creation
  useEffect(() => {
    if (surveyLayerRef.current) {
      surveyLayerRef.current.setActiveTool(activeTool);
    }
  }, [activeTool, mapLoaded]);

  // Set map zoom limits from config
  useEffect(() => {
    if (!map.current || !config) return;
    map.current.setMinZoom(config.map.zoom_min ?? 0);
    map.current.setMaxZoom(config.map.zoom_max ?? 22);
  }, [config]);

  // Track Ctrl key for cursor updates and zoom-box cancellation
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Recover from a genuine Ctrl release that the keyup workaround below
      // swallowed. That suppression cannot tell a spurious keyup from a real one at
      // the moment it fires - but the two diverge immediately afterwards: with the
      // key still physically down every later event reports `ctrlKey: true`, so an
      // event reporting `false` is proof the key is up. Without this, one release
      // timed inside the 50ms window latched `ctrlHeld` on until the user pressed
      // and released Ctrl again - leaving the pan grab over every tool they
      // switched to, since no further keyup was ever coming.
      if (!e.ctrlKey) setCtrlHeld(false);
      if (e.key === 'Control' && !e.repeat) {
        setCtrlHeld(true);

        // Cancel any active zoom box when Ctrl is pressed (mid-drag Ctrl converts to pan)
        setZoomBox({ start: null, end: null });
      }
    };
    const handleKeyUp = (e: KeyboardEvent) => {
      // Same recovery as on keydown: releasing any OTHER key also proves whether
      // Ctrl is still down.
      if (e.key !== 'Control' && !e.ctrlKey) setCtrlHeld(false);
      if (e.key === 'Control') {
        // WORKAROUND: Tauri/WebKit fires spurious Ctrl keyup events at the moment of
        // mousedown/mouseup while holding Ctrl. Only suppress keyups that occur within
        // 50ms of a mouse event during a Ctrl+drag - genuine Ctrl releases go through
        // immediately so the cursor resets without delay.
        if (ctrlPanActiveRef.current && Date.now() - ctrlPanMouseEventTimeRef.current < 50) {
          return;
        }
        setCtrlHeld(false);
      }
    };
    // Also handle window blur (Ctrl held, switch windows, release outside)
    const handleBlur = () => {
      customPanActiveRef.current = false;
      ctrlPanActiveRef.current = false;
      setCtrlHeld(false);
      // Cancel any active zoom box
      setZoomBox({ start: null, end: null });
    };

    /**
     * The same correction from the POINTER, which is the half the keyboard cannot
     * cover: after a Ctrl+drag whose release landed inside the suppression window,
     * the next thing a user does is usually click a toolbar tool or just move the
     * mouse - neither of which is a key event, so `ctrlHeld` stayed latched and the
     * pan grab followed them onto the tool they had just chosen.
     *
     * Deliberately NOT skipped while a Ctrl+pan is running. Exempting that window
     * looked safer and was not: the release is swallowed precisely at the end of a
     * drag, so a click landing inside the same 50ms - reaching straight for the
     * toolbar, which is the whole scenario - was exempted too, and the flag stayed
     * latched until some later stray mouse move happened to clear it. The quirk
     * being worked around is spurious KEYUPS, not mis-reported mouse modifiers; the
     * Ctrl+pan mouseup handler already trusts `e.ctrlKey` the same way. And the flag
     * only drives the cursor - the pan itself runs off `ctrlPanActiveRef` - so even
     * on a platform that did lie here the cost is a cursor, not a broken gesture.
     *
     * Re-setting `false` when it is already `false` is free: React bails out of the
     * render when the state is unchanged, which is the common case on every move.
     */
    const clearIfReleased = (e: MouseEvent | PointerEvent) => {
      if (!e.ctrlKey) setCtrlHeld(false);
    };

    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);
    window.addEventListener('blur', handleBlur);
    window.addEventListener('pointerdown', clearIfReleased);
    window.addEventListener('mousemove', clearIfReleased);

    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
      window.removeEventListener('blur', handleBlur);
      window.removeEventListener('pointerdown', clearIfReleased);
      window.removeEventListener('mousemove', clearIfReleased);
    };
  }, []);

  // Escape key cancels zoom box drag
  useEffect(() => {
    if (!zoomBox.start) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setZoomBox({ start: null, end: null });
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [zoomBox.start]);

  // Escape key cancels move drag operations (station, segment, shot)
  useEffect(() => {
    if (!moveDrag) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        // Clean up drag preview (temp layers, filters, sources)
        surveyLayerRef.current?.endDragPreview();
        // Clear all drag-related refs
        snapMergeDialogPendingRef.current = false;
        dragOriginStationRef.current = null;
        lastRawDragOffsetRef.current = null;
        stationDragColocatedRefsRef.current = [];
        dragConnectedSectionIdsRef.current = new Set();
        moveDragLngLatRef.current = null;
        lastCursorClientRef.current = null;
        dragSnapRef.current = null;
        segmentDragTokenRef.current++;
        setSnappedStation(null);
        setMoveDrag(null);
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [moveDrag, setSnappedStation]);

  // Custom temporary panning implementation for Ctrl+left-drag and middle-drag.
  // MapLibre cannot do either itself: its mouse-pan handler ignores any press
  // that is not a bare left button - maplibre-gl 5.16 `generateMousePanHandler`
  // uses `checkCorrectEvent: (e) => mouseButton(e) === LEFT_BUTTON && !e.ctrlKey`
  // - and with `dragRotate`/`boxZoom` disabled (map init) nothing else claims a
  // Ctrl+left drag either. So we track the movement and call panBy() ourselves,
  // which is also what makes `pan_speed` a thing the modifier gestures can honour.
  const rawPanSpeed = config?.map.controls.pan_speed ?? 1.0;
  const panSpeed = Number.isFinite(rawPanSpeed) ? Math.min(5.0, Math.max(0.1, rawPanSpeed)) : 1.0;
  useEffect(() => {
    const mapElement = mapContainer.current;
    if (!mapElement) return;

    // Track drag start state for custom panning.
    let customPanStart: { x: number; y: number } | null = null;
    let customPanButtonMask: number | null = null;
    // The press point, and whether the gesture has actually been TAKEN OVER.
    // The two differ only for a Ctrl+left press in the modes where a plain drag
    // already pans: there the press is merely WATCHED until it travels far
    // enough to be a drag. Claiming it at mousedown is what made the documented
    // Ctrl+click selection toggle unreachable - the press was swallowed and so
    // was the click behind it, so `handleStationClick`'s `ctrlKey` branch
    // (useMapTools) never ran under any tool.
    let customPanOrigin: { x: number; y: number } | null = null;
    let customPanClaimed = false;

    /** Take the gesture over: suppress it, flag it, and show the pan cursor. */
    const claimCustomPan = (e: MouseEvent, isCtrlLeftPan: boolean) => {
      if (!map.current) return;
      customPanClaimed = true;
      customPanActiveRef.current = true;
      ctrlPanActiveRef.current = isCtrlLeftPan;
      if (isCtrlLeftPan) {
        ctrlPanMouseEventTimeRef.current = Date.now();
      }

      // Suppress default browser behavior during custom pan
      e.stopPropagation();
      e.preventDefault();

      // Change cursor to indicate panning
      map.current.getCanvas().style.cursor = 'grabbing';
    };

    const handleMouseDownCapture = (e: MouseEvent) => {
      const isCtrlLeftPan = e.ctrlKey && e.button === 0;
      const isMiddlePan = e.button === 1;
      // Every tool but pan: the pan tool owns plain left-drag itself, so the
      // modifier gesture would be redundant there.
      if ((isCtrlLeftPan || isMiddlePan) && activeTool !== 'pan' && map.current) {
        customPanStart = { x: e.clientX, y: e.clientY };
        customPanOrigin = { x: e.clientX, y: e.clientY };
        customPanButtonMask = isMiddlePan ? 4 : 1;
        customPanClaimed = false;

        // Claim the press outright where nothing else can want it: a middle
        // press has no other meaning, and where MapLibre's dragPan is OFF the
        // mode owns the press itself (placement confirms on pointerdown, the
        // zoom tool starts its box) - neither of which may happen because Ctrl
        // was held. Where a plain drag already pans, a Ctrl+left press is only
        // watched: see `customPanClaimed`. Carrying a move ghost is in that
        // second group since #409 - the drop is click-gated, so a Ctrl press
        // that travels becomes a pan on the same 3px threshold as everywhere
        // else, and one that does not travel drops the ghost.
        if (isMiddlePan || !mapLibreDragPan) {
          claimCustomPan(e, isCtrlLeftPan);
        }
      }
    };

    // Implement custom panning since modified/middle drag interactions are handled manually.
    const handleMouseMoveCapture = (e: MouseEvent) => {
      if (customPanStart && customPanButtonMask !== null && (e.buttons & customPanButtonMask) !== 0 && map.current) {
        if (!customPanClaimed) {
          // A watched Ctrl+left press becomes a pan the moment it stops being a
          // click. The threshold is MapLibre's own `clickTolerance` (default 3px,
          // map init does not override it), so the handover lands exactly where
          // MapLibre itself stops calling the gesture a click: below it the press
          // is left alone and the click goes through to the selection toggle,
          // above it MapLibre would suppress its own click anyway.
          if (!customPanOrigin) return;
          const travel = Math.hypot(e.clientX - customPanOrigin.x, e.clientY - customPanOrigin.y);
          if (travel < CUSTOM_PAN_CLAIM_PX) return;
          // Pans from the PRESS point, not from here: the map follows the whole
          // travel, so the takeover is invisible rather than a 3px jump.
          claimCustomPan(e, true);
        }
        const dx = e.clientX - customPanStart.x;
        const dy = e.clientY - customPanStart.y;

        // Pan the map by the delta (negative because we're dragging the "ground").
        // pan_speed scales this modifier-drag pan only: the pan TOOL's dragPan is
        // direct manipulation (map glued to cursor), so a multiplier there would
        // read as broken tracking, but Ctrl/middle drag is already a shortcut
        // gesture where sensitivity is a preference.
        if (Math.abs(dx) > 1 || Math.abs(dy) > 1) {
          map.current.panBy([-dx * panSpeed, -dy * panSpeed], { animate: false });
          customPanStart = { x: e.clientX, y: e.clientY };
        }
        if (ctrlPanActiveRef.current) {
          ctrlPanMouseEventTimeRef.current = Date.now();
        }
        // Stop propagation to prevent MapLibre from updating hover state during pan
        e.stopPropagation();
        e.preventDefault();
      }
    };

    /**
     * Restore cursor after a claimed pan: if Ctrl is still held, show the grab
     * cursor for the next potential pan; otherwise restore the tool cursor.
     * Hand it to SurveyLayer rather than writing the canvas directly, exactly as
     * the passive effect above does - it owns the hover-vs-tool precedence,
     * including Move Station/Shot's arrow fallback, which a raw `move` write
     * here would defeat with nothing to correct it (hover only re-derives when
     * the hovered id CHANGES, so a pointer resting on empty map keeps it).
     */
    const restoreCursorAfterPan = (e: MouseEvent) => {
      if (!map.current) return;
      const ctrlStillPanning = e.ctrlKey && activeTool !== 'pan';
      const restored = ctrlStillPanning ? 'grab' : toolCursor(activeTool);
      if (surveyLayerRef.current) {
        surveyLayerRef.current.setBaseCursor(restored, ctrlStillPanning);
      } else {
        map.current.getCanvas().style.cursor = restored;
      }
    };

    const handleMouseUpCapture = (e: MouseEvent) => {
      if (customPanStart && map.current) {
        const wasClaimed = customPanClaimed;
        customPanStart = null;
        customPanButtonMask = null;
        customPanOrigin = null;
        customPanClaimed = false;
        // A watched press that never travelled is a Ctrl+CLICK, not a pan. None
        // of the suppression below may touch it - and `customPanActiveRef`, which
        // the click capture reads, was never set, so the click reaches MapLibre.
        if (!wasClaimed) return;
        // Record timestamp for spurious keyup filter (Tauri/WebKit can fire one near mouseup too)
        if (ctrlPanActiveRef.current) {
          ctrlPanMouseEventTimeRef.current = Date.now();
        }
        // Suppress default browser behavior; the Ctrl+click guard in
        // SurveyLayer.handleMapClick prevents move/select actions
        e.stopPropagation();
        e.preventDefault();
        // Clear custom pan flags after a small delay (can't rely on handleMouseUpGlobal
        // because stopPropagation prevents it from firing on window bubble)
        setTimeout(() => {
          customPanActiveRef.current = false;
          ctrlPanActiveRef.current = false;
        }, 50);
        restoreCursorAfterPan(e);
      }
    };

    // Capture the DOM click event during/after a custom pan gesture.
    // The browser fires 'click' as a separate event after mouseup - stopping propagation
    // on mousedown/mouseup does NOT prevent click from being dispatched. MapLibre listens
    // for 'click' on the canvas, so we must capture it to prevent spurious map clicks.
    const handleClickCapture = (e: MouseEvent) => {
      if (customPanActiveRef.current || placementJustConfirmedRef.current) {
        e.stopPropagation();
        e.preventDefault();
      }
    };

    // Capture pointer events during custom pan - MapLibre may use pointer events
    // internally, so capturing only mouse events is insufficient to prevent click leakage
    const handlePointerDownCapture = (e: PointerEvent) => {
      const isCtrlLeftPan = e.ctrlKey && e.button === 0;
      const isMiddlePan = e.button === 1;
      // Mirrors the outright claim in handleMouseDownCapture. A merely WATCHED
      // Ctrl press must reach the app's own pointerdown handling untouched - in
      // those modes it does nothing with it anyway (placement and the zoom box
      // are both modal), and stopping it here would be a second way to swallow
      // the gesture behind the mousedown capture's back.
      if ((isMiddlePan || (isCtrlLeftPan && !mapLibreDragPan)) && activeTool !== 'pan' && map.current && e.pointerType === 'mouse') {
        e.stopPropagation();
      }
    };

    const handlePointerUpCapture = (e: PointerEvent) => {
      // If we're in custom pan, stop pointer events from reaching MapLibre.
      // (customPanStart is still set here - cleared by handleMouseUpCapture which fires after)
      if (customPanStart && customPanClaimed && e.pointerType === 'mouse') {
        e.stopPropagation();
      }
    };

    // Track if mouse is currently down (for filtering spurious keyup)
    let mouseIsDown = false;

    const handleMouseDownGlobal = () => {
      mouseIsDown = true;
    };

    const handleMouseUpGlobal = (e: MouseEvent) => {
      mouseIsDown = false;
      // Clear pan state in case mouse was released outside the map container
      // (handleMouseUpCapture only fires for events on the container element)
      const wasClaimed = customPanClaimed;
      customPanStart = null;
      customPanButtonMask = null;
      customPanOrigin = null;
      customPanClaimed = false;
      if (ctrlPanActiveRef.current) {
        ctrlPanMouseEventTimeRef.current = Date.now();
      }
      // The pan now runs past the container edge, so releasing outside it is an
      // ordinary way to end one, and this is the only handler that sees it -
      // the grabbing cursor has to be undone here too or it sticks.
      if (wasClaimed) {
        restoreCursorAfterPan(e);
      }
      // Fallback: clear custom pan flags for mouseups outside the map container
      // (handleMouseUpCapture handles the in-container case)
      if (customPanActiveRef.current) {
        setTimeout(() => {
          if (!mouseIsDown) {
            customPanActiveRef.current = false;
            ctrlPanActiveRef.current = false;
          }
        }, 50);
      }
    };

    // Use capture: true to intercept BEFORE MapLibre's handlers
    mapElement.addEventListener('mousedown', handleMouseDownCapture, { capture: true });
    // The moves go on WINDOW, not the map element: a drag that leaves the
    // element still has to steer the camera, and an element listener simply
    // stops receiving moves once the cursor is outside it - a 300px gesture
    // delivered the 135px to the edge and then froze (#429). The handler
    // no-ops unless `customPanStart` is set, which only a mousedown on the map
    // element does, so widening the listener does not widen what it acts on;
    // and because it suppresses at window-capture, MapLibre still never sees
    // the moves it would have taken from the element listener.
    window.addEventListener('mousemove', handleMouseMoveCapture, { capture: true });
    mapElement.addEventListener('mouseup', handleMouseUpCapture, { capture: true });
    mapElement.addEventListener('click', handleClickCapture, { capture: true });
    mapElement.addEventListener('pointerdown', handlePointerDownCapture, { capture: true });
    mapElement.addEventListener('pointerup', handlePointerUpCapture, { capture: true });
    window.addEventListener('mousedown', handleMouseDownGlobal, { capture: true });
    window.addEventListener('mouseup', handleMouseUpGlobal);
    // A plain left-drag - the default pan gesture since #409 - released over the
    // floating chrome (dock reveal handles, the Data tab, MapLibre's own
    // navigation/scale controls) would otherwise leave the map stuck "moving";
    // src/utils/mapDragRelease.ts has why MapLibre needs the help.
    const detachDragReleaseForwarder = attachDragReleaseForwarder(() => map.current);

    return () => {
      mapElement.removeEventListener('mousedown', handleMouseDownCapture, { capture: true });
      window.removeEventListener('mousemove', handleMouseMoveCapture, { capture: true });
      mapElement.removeEventListener('mouseup', handleMouseUpCapture, { capture: true });
      mapElement.removeEventListener('click', handleClickCapture, { capture: true });
      mapElement.removeEventListener('pointerdown', handlePointerDownCapture, { capture: true });
      mapElement.removeEventListener('pointerup', handlePointerUpCapture, { capture: true });
      window.removeEventListener('mousedown', handleMouseDownGlobal, { capture: true });
      window.removeEventListener('mouseup', handleMouseUpGlobal);
      detachDragReleaseForwarder();
    };
    // `mapLibreDragPan` decides whether a Ctrl+left press is claimed outright or
    // only watched, so the handlers have to be re-registered when it flips. That
    // re-registers the drag-release forwarder with them; it is attach/detach
    // symmetric and holds no in-flight state of its own, so re-arming it between
    // gestures is inert.
  }, [activeTool, mapLibreDragPan, panSpeed]);

  // Zoom tool capture handlers - intercept clicks on the map canvas to prevent SurveyLayer selection
  // and handle right-click zoom out behavior. Attaches to the canvas element specifically to avoid
  // breaking MapLibre UI controls (NavigationControl, ScaleControl, etc.)
  useEffect(() => {
    if (!map.current || activeTool !== 'zoom') return;

    const canvas = map.current.getCanvas();
    if (!canvas) return;

    // Left-click capture: stop propagation to prevent SurveyLayer selection
    // Actual zoom is handled by zoom box end handler (handleZoomBoxEnd)
    // Don't intercept Ctrl+click (used for Ctrl+drag panning)
    // Don't intercept during placement mode (let placement click through)
    const handleClickCapture = (e: MouseEvent) => {
      if (placementMode) return;
      if (e.button === 0 && !e.ctrlKey) {
        e.stopPropagation();
        e.stopImmediatePropagation();
        // Zoom-in is handled by handleZoomBoxEnd for <10px drags
      }
    };

    // Right-click capture: cancel zoom box if active, otherwise zoom out at click point
    const handleContextMenuCapture = (e: MouseEvent) => {
      // Don't intercept during placement mode
      if (placementMode) return;

      e.preventDefault();
      e.stopPropagation();
      e.stopImmediatePropagation();

      // If a zoom box is in progress, cancel it instead of zooming
      if (zoomBox.start) {
        setZoomBox({ start: null, end: null });
        return;
      }

      if (!map.current || !mapContainer.current) return;

      const currentZoom = map.current.getZoom();
      const targetZoom = Math.max(zoomMin, currentZoom - zoomStep);

      // Early return if at min zoom (no recenter)
      if (targetZoom === currentZoom) return;

      // Convert click to map coords
      const rect = mapContainer.current.getBoundingClientRect();
      const screenX = e.clientX - rect.left;
      const screenY = e.clientY - rect.top;
      const clickLngLat = map.current.unproject([screenX, screenY]);

      map.current.easeTo({
        center: clickLngLat,
        zoom: targetZoom,
        duration: 300,
      });
    };

    canvas.addEventListener('click', handleClickCapture, { capture: true });
    canvas.addEventListener('contextmenu', handleContextMenuCapture, { capture: true });

    return () => {
      canvas.removeEventListener('click', handleClickCapture, { capture: true });
      canvas.removeEventListener('contextmenu', handleContextMenuCapture, { capture: true });
    };
  }, [activeTool, zoomStep, zoomMin, placementMode, zoomBox.start]);

  // Get satellite tile URL based on provider config
  // Uses the custom stalagmight-tiles:// protocol; the backend caches under the
  // provider's own Cache-Control
  const getSatelliteTileConfig = useCallback(() => {
    return getTileConfig();
  }, []);

  // Handle satellite toggle
  useEffect(() => {
    if (!map.current || !mapLoaded) return;

    // Store reference for cleanup
    const currentMap = map.current;
    let errorHandler: ((e: maplibregl.ErrorEvent & { sourceId?: string }) => void) | null = null;

    if (showSatellite) {
      // Add satellite layer if not already present
      if (!currentMap.getSource('satellite-tiles')) {
        const tileConfig = getSatelliteTileConfig();

        currentMap.addSource('satellite-tiles', {
          type: 'raster',
          tiles: tileConfig.tiles,
          tileSize: tileConfig.tileSize,
          attribution: tileConfig.attribution,
          minzoom: tileConfig.minZoom,
          maxzoom: tileConfig.maxZoom,
        });

        // Satellite draws below the grid and both below the survey; the shared
        // candidate list owns that ordering. An unresolved anchor would append
        // the raster over everything, so it must not be the common case.
        const bottomAnchor = SATELLITE_BOTTOM_CANDIDATES.find((id) => currentMap.getLayer(id));
        currentMap.addLayer({
          id: 'satellite-tiles',
          type: 'raster',
          source: 'satellite-tiles',
          paint: {
            'raster-opacity': 0.7,
          },
        }, bottomAnchor);

        // Add error handler for tile fetch failures
        const sourceId = 'satellite-tiles';
        errorHandler = (e: maplibregl.ErrorEvent & { sourceId?: string }) => {
          if (e.sourceId === sourceId) {
            console.warn('[MapCanvas] Satellite tile load failed - the imagery provider could not be reached');
            // The map will gracefully show the background color where tiles fail to load
          }
        };

        currentMap.on('error', errorHandler);
      }
    } else {
      if (currentMap.getLayer('satellite-tiles')) {
        currentMap.removeLayer('satellite-tiles');
      }
      if (currentMap.getSource('satellite-tiles')) {
        currentMap.removeSource('satellite-tiles');
      }
    }

    // Cleanup function
    return () => {
      if (errorHandler) {
        currentMap.off('error', errorHandler);
      }
    };
  }, [showSatellite, getSatelliteTileConfig, mapLoaded]);

  // Reload stale satellite tiles after offline→online recovery
  const tileReloadGeneration = useNetworkStore((s) => s.tileReloadGeneration);
  useEffect(() => {
    if (!map.current || !mapLoaded || !showSatellite || tileReloadGeneration === 0) return;
    const source = map.current.getSource('satellite-tiles');
    if (source && 'clearTiles' in source) {
      (source as any).clearTiles();
      map.current.triggerRepaint();
    }
  }, [tileReloadGeneration, mapLoaded, showSatellite]);

  // 10% margins on all sides. The map is the central flex column (already
  // sized to exclude the side panels), so no panel offsets are needed - adding
  // them double-counts the panel widths and squeezes the fit (see #64).
  const calcPadding = useCallback((container: HTMLElement) => {
    const marginX = Math.round(container.clientWidth * 0.1);
    const marginY = Math.round(container.clientHeight * 0.1);
    return { top: marginY, bottom: marginY, left: marginX, right: marginX };
  }, []);

  // Fit to survey bounds using transformed coordinates
  const fitToBounds = useCallback((options?: { snap?: boolean }) => {
    // Frame the *active* (drawn) geometry - adjusted in the closure-adjusted view.
    if (!map.current) return;

    // Prefer the SoA `stationCoords` typed array (stride-3 [x,y,z,…]) so the
    // fit computes X/Y bounds without materializing the ~89k-object lazy
    // `activeStations` view - this callback runs on every load via the
    // requestFitToBounds effect. Fall back to `activeStations` only when
    // surveyData isn't available yet (unusual early-init paths).
    let bounds: { minX: number; maxX: number; minY: number; maxY: number };
    const coordsArr = activeSurveyData?.stationCoords;
    if (coordsArr && coordsArr.length > 0) {
      let minX = Infinity;
      let maxX = -Infinity;
      let minY = Infinity;
      let maxY = -Infinity;
      for (let i = 0; i + 2 < coordsArr.length; i += 3) {
        const x = coordsArr[i]!;
        const y = coordsArr[i + 1]!;
        if (x < minX) minX = x;
        if (x > maxX) maxX = x;
        if (y < minY) minY = y;
        if (y > maxY) maxY = y;
      }
      bounds = { minX, maxX, minY, maxY };
    } else {
      if (activeStations.length === 0) return;
      const coords = activeStations.map((s) => s.coordinates);
      bounds = getBounds(coords);
    }

    // Transform bounds to map coordinates
    const sw = metersToMapCoords(bounds.minX, bounds.minY, gpsOrigin);
    const ne = metersToMapCoords(bounds.maxX, bounds.maxY, gpsOrigin);

    // Reset padding to avoid MapLibre accumulation bug
    map.current.setPadding({ top: 0, right: 0, bottom: 0, left: 0 });

    // Calculate 10% padding based on container size (no panel offsets)
    const container = map.current.getContainer();
    const padding = calcPadding(container);

    // Respect the configured animation speed (matches fitToSection / center moves).
    // `snap` forces an instant jump regardless of the setting.
    const animationConfig = config?.map.animation;
    const animationDuration = animationConfig && !animationConfig.enabled ? 0 : (animationConfig?.duration ?? 300);

    // Use MapLibre's fitBounds with percentage-based padding
    map.current.fitBounds([sw, ne], {
      padding,
      maxZoom: 20,
      ...(options?.snap ? { animate: false } : { duration: animationDuration }),
    });
  }, [activeStations, activeSurveyData, gpsOrigin, calcPadding, config]);

  // Fit to a specific section's bounds
  const fitToSection = useCallback(
    (sectionId: number) => {
      if (!map.current || !config) return;

      // Filter stations belonging to this section
      const sectionStations = activeStations.filter((s) => s.station_ref.section_id === sectionId);

      // Empty section: nothing to zoom to
      if (sectionStations.length === 0) {
        return;
      }

      // Fit to *rendered* geometry: endpoints of drawn shots plus zero-datum
      // stations, falling back to all stations when nothing matches. A hidden
      // connector or a station only a hidden shot touches shouldn't inflate the
      // bounds and shrink the section; a *visible* one must be included, or it
      // could be left off-screen. Jumps and References are independent toggles
      // (#27), so this is per-kind - judging both by the reference toggle alone
      // got it backwards in each direction.
      const byIndex = new Map<number, (typeof sectionStations)[number]>();
      for (const s of sectionStations) byIndex.set(s.station_ref.station_index, s);
      const section = project?.sections.find((s) => s.id === sectionId);
      const rendered: typeof sectionStations = [];
      const seenIdx = new Set<number>();
      const includeIdx = (idx: number) => {
        if (seenIdx.has(idx)) return;
        const st = byIndex.get(idx);
        if (st) {
          rendered.push(st);
          seenIdx.add(idx);
        }
      };
      section?.shots.forEach((shot, i) => {
        if (!isShotKindVisible(shot.kind, showShots, showJumps, showReferenceShots)) return;
        includeIdx(i);
        includeIdx(i + 1);
      });
      for (const s of sectionStations) {
        if (s.is_zero_datum) includeIdx(s.station_ref.station_index);
      }
      const fitStations = rendered.length > 0 ? rendered : sectionStations;

      // Stop any running animation before starting a new one
      map.current.stop();

      // Reset padding to avoid MapLibre accumulation bug
      map.current.setPadding({ top: 0, right: 0, bottom: 0, left: 0 });

      // Get animation config
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;

      // Padding: 10% margins only. The map container is the central flex
      // column, so it already excludes the side panels - adding the panel
      // widths here double-counts and squeezes the section into a sliver
      // (e.g. a 730px map minus 250+300px panels left ~34px usable). Matches
      // fitToBounds.
      const container = map.current.getContainer();
      const padding = calcPadding(container);

      // Single station: center at point_zoom level
      if (fitStations.length === 1) {
        const station = fitStations[0]!;
        const mapCoords = metersToMapCoords(station.coordinates.x, station.coordinates.y, gpsOrigin);

        map.current.easeTo({
          center: mapCoords as [number, number],
          zoom: animationConfig.point_zoom,
          duration: animationDuration,
          padding,
        });
        return;
      }

      // Multiple stations: calculate bounds and fit
      const coords = fitStations.map((s) => s.coordinates);
      const bounds = getBounds(coords);

      // Handle zero-area bounds (all stations at same XY) like single-station case
      if (bounds.minX === bounds.maxX && bounds.minY === bounds.maxY) {
        const mapCoords = metersToMapCoords(bounds.minX, bounds.minY, gpsOrigin);
        map.current.easeTo({
          center: mapCoords as [number, number],
          zoom: animationConfig.point_zoom,
          duration: animationDuration,
          padding,
        });
        return;
      }

      // Transform bounds to map coordinates (pass gpsOrigin for satellite alignment)
      const sw = metersToMapCoords(bounds.minX, bounds.minY, gpsOrigin);
      const ne = metersToMapCoords(bounds.maxX, bounds.maxY, gpsOrigin);

      // Always fit the section's bounds. fitBounds depends only on the bounds,
      // padding and container - not the live (possibly mid-animation) camera -
      // so rapidly clicking between different sections can't drift the zoom.
      // (The previous skip-zoom-in heuristic read the transitional camera via
      // map.project() and accumulated across interrupted animations.) Re-fitting
      // an already-framed section is idempotent (same target zoom).
      map.current.fitBounds([sw, ne], {
        padding,
        linear: true,
        duration: animationDuration,
        maxZoom: config.map.zoom_max ?? 22,
      });
    },
    [activeStations, project?.sections, config, gpsOrigin, showShots, showJumps, showReferenceShots, calcPadding]
  );

  // Pan to a shot's midpoint (used when double-clicking a row in DataTable)
  const panToShotMidpoint = useCallback(
    (sectionId: SectionId, shotId: ShotId) => {
      if (!map.current || !config || !project) return;

      // Find the section
      const section = project.sections.find((s) => s.id === sectionId);
      if (!section) return;

      // Find the shot index (shot position - 1, since shots connect stations)
      const shotIndex = section.shots.findIndex((s) => s.id === shotId);
      if (shotIndex === -1) return;

      // Shot at index N connects station_index N (from) to station_index N+1 (to)
      const fromStationIndex = shotIndex;
      const toStationIndex = shotIndex + 1;

      // Find the two stations
      const fromStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === fromStationIndex
      );
      const toStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === toStationIndex
      );

      if (!fromStation || !toStation) return;

      // Calculate midpoint in project coordinates (meters)
      const midpointX = (fromStation.coordinates.x + toStation.coordinates.x) / 2;
      const midpointY = (fromStation.coordinates.y + toStation.coordinates.y) / 2;

      // Convert to map coordinates
      const mapCoords = metersToMapCoords(midpointX, midpointY, gpsOrigin);

      // Get animation config
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;

      // Pan to the midpoint with smooth animation
      map.current.easeTo({
        center: mapCoords as [number, number],
        duration: animationDuration,
      });
    },
    [project, activeStations, config, gpsOrigin]
  );

  // Progressive zoom-in centered on a shot's midpoint: each call recenters and
  // zooms one step closer (capped at zoom_max). Drives the resolve view's zoom
  // button (#57) - clicking repeatedly closes in on the problematic shot.
  const zoomToShotMidpoint = useCallback(
    (sectionId: SectionId, shotId: ShotId) => {
      if (!map.current || !config || !project) return;
      const section = project.sections.find((s) => s.id === sectionId);
      if (!section) return;
      const shotIndex = section.shots.findIndex((s) => s.id === shotId);
      if (shotIndex === -1) return;
      const fromStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === shotIndex,
      );
      const toStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === shotIndex + 1,
      );
      if (!fromStation || !toStation) return;
      const midpointX = (fromStation.coordinates.x + toStation.coordinates.x) / 2;
      const midpointY = (fromStation.coordinates.y + toStation.coordinates.y) / 2;
      const mapCoords = metersToMapCoords(midpointX, midpointY, gpsOrigin);
      const currentZoom = map.current.getZoom();
      const targetZoom = Math.min(zoomMax, currentZoom + zoomStep);
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;
      map.current.easeTo({
        center: mapCoords as [number, number],
        zoom: targetZoom,
        duration: animationDuration,
      });
    },
    [project, activeStations, config, gpsOrigin, zoomMax, zoomStep],
  );

  // Center on a station (used by DetailLink)
  // Returns true if centering was performed, false if prerequisites not met
  const fitToStation = useCallback(
    (stationRef: StationRef): boolean => {
      if (!map.current || !config) return false;

      // Find the station
      const station = activeStations.find(
        (s) =>
          s.station_ref.section_id === stationRef.section_id &&
          s.station_ref.station_index === stationRef.station_index
      );
      if (!station) return false;

      // Stop any running animation before starting a new one
      map.current.stop();

      // Reset padding to avoid MapLibre accumulation bug (follows fitToSection pattern)
      map.current.setPadding({ top: 0, right: 0, bottom: 0, left: 0 });

      // Convert to map coordinates
      const mapCoords = metersToMapCoords(station.coordinates.x, station.coordinates.y, gpsOrigin);

      // Get animation config
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;

      // Center on the station using point_zoom (single point centering, no padding needed)
      map.current.easeTo({
        center: mapCoords as [number, number],
        zoom: animationConfig.point_zoom,
        duration: animationDuration,
      });

      return true;
    },
    [activeStations, config, gpsOrigin]
  );

  // Center on a shot's midpoint (used by DetailLink)
  // Returns true if centering was performed, false if prerequisites not met
  const fitToShot = useCallback(
    (sectionId: SectionId, shotId: ShotId): boolean => {
      if (!map.current || !config || !project) return false;

      // Find the section
      const section = project.sections.find((s) => s.id === sectionId);
      if (!section) return false;

      // Find the shot index
      const shotIndex = section.shots.findIndex((s) => s.id === shotId);
      if (shotIndex === -1) return false;

      // Shot at index N connects station_index N (from) to station_index N+1 (to)
      const fromStationIndex = shotIndex;
      const toStationIndex = shotIndex + 1;

      // Find the two stations
      const fromStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === fromStationIndex
      );
      const toStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === toStationIndex
      );

      if (!fromStation || !toStation) return false;

      // Stop any running animation before starting a new one
      map.current.stop();

      // Reset padding to avoid MapLibre accumulation bug (follows fitToSection pattern)
      map.current.setPadding({ top: 0, right: 0, bottom: 0, left: 0 });

      // Calculate midpoint in project coordinates (meters)
      const midpointX = (fromStation.coordinates.x + toStation.coordinates.x) / 2;
      const midpointY = (fromStation.coordinates.y + toStation.coordinates.y) / 2;

      // Convert to map coordinates
      const mapCoords = metersToMapCoords(midpointX, midpointY, gpsOrigin);

      // Get animation config
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;

      // Center on the shot midpoint using point_zoom (single point centering, no padding needed)
      map.current.easeTo({
        center: mapCoords as [number, number],
        zoom: animationConfig.point_zoom,
        duration: animationDuration,
      });

      return true;
    },
    [project, activeStations, config, gpsOrigin]
  );

  // Fit to bounds when requested (e.g., after import)
  useEffect(() => {
    if (requestFitToBounds && mapLoaded && stations.length > 0) {
      const snap = requestFitToBounds === 'snap';
      if (snap) {
        fitToBounds({ snap: true });
        // The load's FIRST feature build can run against the pre-fit camera;
        // if that transient zoom sat above the label build threshold while
        // the fitted camera lands inside the hysteresis band, ~165k label
        // features would persist that a load at the final camera never
        // builds. Re-evaluate from a clean slate at the fitted camera (#mem).
        surveyLayerRef.current?.resetLabelZoomHysteresis();
        clearFitToBoundsRequest();
      } else {
        // Small delay to ensure data is rendered
        setTimeout(() => {
          fitToBounds();
          // Same clean-slate label re-evaluation as the snap path, but the
          // camera animates here - wait for the map to settle ('idle' also
          // fires when the fit was a no-op, unlike 'moveend').
          map.current?.once('idle', () => {
            surveyLayerRef.current?.resetLabelZoomHysteresis();
          });
          clearFitToBoundsRequest();
        }, 100);
      }
    }
  }, [requestFitToBounds, mapLoaded, stations.length, fitToBounds, clearFitToBoundsRequest]);

  // Fit to section when requested (e.g., from sections panel double-click)
  useEffect(() => {
    // CRITICAL: Clear request immediately (before any early returns) to prevent stuck requests
    if (requestZoomToSection !== null) {
      const sectionId = requestZoomToSection;
      clearZoomToSectionRequest();
      // If map is loaded, process immediately; otherwise store for later
      if (mapLoaded) {
        fitToSection(sectionId);
      } else {
        pendingZoomToSectionRef.current = sectionId;
      }
    }
  }, [requestZoomToSection, mapLoaded, clearZoomToSectionRequest, fitToSection]);

  // Process pending section zoom request when map loads
  useEffect(() => {
    if (mapLoaded && pendingZoomToSectionRef.current !== null) {
      const sectionId = pendingZoomToSectionRef.current;
      pendingZoomToSectionRef.current = null;
      fitToSection(sectionId);
    }
  }, [mapLoaded, fitToSection]);

  // Pan to shot midpoint when requested (e.g., from DataTable double-click)
  useEffect(() => {
    // CRITICAL: Clear request immediately (before any early returns) to prevent stuck requests
    if (requestPanToShot !== null) {
      const { sectionId, shotId } = requestPanToShot;
      clearPanToShotRequest();
      // If map is loaded, process immediately; otherwise store for later
      if (mapLoaded) {
        panToShotMidpoint(sectionId, shotId);
      } else {
        pendingPanToShotRef.current = { sectionId, shotId };
      }
    }
  }, [requestPanToShot, mapLoaded, clearPanToShotRequest, panToShotMidpoint]);

  // Process pending pan-to-shot request when map loads
  useEffect(() => {
    if (mapLoaded && pendingPanToShotRef.current !== null) {
      const { sectionId, shotId } = pendingPanToShotRef.current;
      pendingPanToShotRef.current = null;
      panToShotMidpoint(sectionId, shotId);
    }
  }, [mapLoaded, panToShotMidpoint]);

  // Center on station when requested (e.g., from DetailLink click)
  useEffect(() => {
    // CRITICAL: Clear request immediately (before any early returns) to prevent stuck requests
    if (requestCenterOnStation !== null) {
      const stationRef = requestCenterOnStation;
      clearCenterOnStationRequest();
      // If map is loaded, process immediately; otherwise store for later
      if (mapLoaded) {
        fitToStation(stationRef);
      } else {
        pendingCenterOnStationRef.current = stationRef;
      }
    }
  }, [requestCenterOnStation, mapLoaded, clearCenterOnStationRequest, fitToStation]);

  // Process pending center-on-station request when map loads
  useEffect(() => {
    if (mapLoaded && pendingCenterOnStationRef.current !== null) {
      const stationRef = pendingCenterOnStationRef.current;
      // Only clear pending ref if centering was successful
      if (fitToStation(stationRef)) {
        pendingCenterOnStationRef.current = null;
      }
    }
  }, [mapLoaded, fitToStation]);

  // Center on shot when requested (e.g., from DetailLink click)
  useEffect(() => {
    // CRITICAL: Clear request immediately (before any early returns) to prevent stuck requests
    if (requestCenterOnShot !== null) {
      const { sectionId, shotId } = requestCenterOnShot;
      clearCenterOnShotRequest();
      // If map is loaded, process immediately; otherwise store for later
      if (mapLoaded) {
        fitToShot(sectionId, shotId);
      } else {
        pendingCenterOnShotRef.current = { sectionId, shotId };
      }
    }
  }, [requestCenterOnShot, mapLoaded, clearCenterOnShotRequest, fitToShot]);

  // Render the transient geometry preview (issue #57): the warning-resolution
  // flow pushes a proposed ghost shot here as the user tries options, drawn via
  // the survey layer's ghost overlay (setGhostSection).
  useEffect(() => {
    if (!mapLoaded) return;
    surveyLayerRef.current?.setGhostSection(geometryPreview ?? null);
  }, [geometryPreview, mapLoaded]);

  // Progressive zoom-in onto a shot (resolve view's zoom button, #57). Each
  // request recenters and zooms one step closer.
  useEffect(() => {
    if (requestZoomToShot !== null) {
      const { sectionId, shotId } = requestZoomToShot;
      clearZoomToShotRequest();
      if (mapLoaded) zoomToShotMidpoint(sectionId, shotId);
    }
  }, [requestZoomToShot, mapLoaded, clearZoomToShotRequest, zoomToShotMidpoint]);

  // Process pending center-on-shot request when map loads
  useEffect(() => {
    if (mapLoaded && pendingCenterOnShotRef.current !== null) {
      const { sectionId, shotId } = pendingCenterOnShotRef.current;
      // Only clear pending ref if centering was successful
      if (fitToShot(sectionId, shotId)) {
        pendingCenterOnShotRef.current = null;
      }
    }
  }, [mapLoaded, fitToShot]);

  // Seed the to-scale placement factor the moment placement mode turns on. The
  // map's 'zoom' handler keeps it live from there, but it does not fire until
  // the user actually zooms, so without this the overlay would draw at whatever
  // scale was last recorded (or the initial 1 px/m) until the first zoom.
  useEffect(() => {
    if (!placementActive || !mapLoaded || !map.current) return;
    setPixelsPerMeter(pixelsPerMeterAt(map.current));
  }, [placementActive, mapLoaded]);

  // Handle Escape key for placement mode
  useEffect(() => {
    if (!placementMode) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        handlePlacementCancel();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [placementMode, handlePlacementCancel]);

  // Update cursor style for placement mode
  useEffect(() => {
    if (!map.current) return;

    if (placementMode) {
      map.current.getCanvas().style.cursor = 'crosshair';
    } else {
      // Reset cursor based on current tool. This effect keys on `activeTool` as
      // well as `placementMode`, so it also runs on an ordinary tool switch,
      // AFTER the passive effect above (effects run in declaration order) - a
      // raw write here therefore lands on top of the precedence that effect just
      // applied. Today the end state is still correct, because a later hover
      // recompute settles it back, but that is luck rather than design: hand it
      // to the same owner instead of racing it.
      if (surveyLayerRef.current) {
        surveyLayerRef.current.setBaseCursor(toolCursor(activeTool));
      } else {
        map.current.getCanvas().style.cursor = toolCursor(activeTool);
      }
    }
  }, [placementMode, activeTool]);

  // `touchAction` - block browser gestures only where the tool or mode owns the
  // drag - comes from the single `mapGestureMode` call above.

  // Combined pointer handlers (PointerEvent is a superset of MouseEvent)
  const handlePointerMove = useCallback(
    (e: React.PointerEvent) => {
      // Handle placement mode
      if (placementMode && mapContainer.current && map.current) {
        const rect = mapContainer.current.getBoundingClientRect();
        const screenX = e.clientX - rect.left;
        const screenY = e.clientY - rect.top;

        // Update cursor position for overlay
        setPlacementCursor({ x: screenX, y: screenY });

        // Convert screen position to map coordinates (meters)
        const lngLat = map.current.unproject([screenX, screenY]);
        const meters = mapCoordsToMeters(lngLat.lng, lngLat.lat, gpsOrigin);
        updatePlacementCursor({ x: meters.x, y: meters.y, z: 0 });

        // Check for snap to existing station
        const snapped = checkPlacementSnap(screenX, screenY);
        updatePlacementSnap(snapped);
        return;
      }

      // A live press that travels past the click threshold stops being a
      // possible drop and becomes a pan, permanently for this gesture: forget
      // the press point so the release cannot commit even if the cursor comes
      // back to it. Latching mirrors the custom pan's `customPanClaimed`, which
      // also never un-claims. Moves with no button down (the ghost simply
      // following the cursor) cannot reach this with a press in flight - a
      // press is only live between its own pointerdown and pointerup.
      const press = pressClientRef.current;
      if (press && Math.hypot(e.clientX - press.x, e.clientY - press.y) >= CUSTOM_PAN_CLAIM_PX) {
        pressClientRef.current = null;
      }

      if (moveDrag) {
        handleMoveDragMove(e.clientX, e.clientY);
      } else if (zoomBox.start) {
        handleZoomBoxMove(e);
      }

      // Always check snap when moving (snap logic internally checks if tool requires snap).
      // During active drag, segment drag has its own snap logic in handleMoveDragMove.
      // RAF-batched: only processes the latest mouse position per animation frame,
      // preventing event queue buildup (same pattern as SurveyLayer hover).
      if (mapContainer.current && !moveDrag) {
        const rect = mapContainer.current.getBoundingClientRect();
        pendingSnapPosition.current = { x: e.clientX - rect.left, y: e.clientY - rect.top };
        if (snapRafId.current === null) {
          snapRafId.current = requestAnimationFrame(() => {
            snapRafId.current = null;
            if (pendingSnapPosition.current) {
              const pos = pendingSnapPosition.current;
              pendingSnapPosition.current = null;
              checkSnapToStation(pos.x, pos.y);
            }
          });
        }
      }
    },
    [moveDrag, zoomBox.start, handleMoveDragMove, handleZoomBoxMove, checkSnapToStation, placementMode, checkPlacementSnap, updatePlacementCursor, updatePlacementSnap]
  );

  const handlePointerUp = useCallback((e: React.PointerEvent) => {
    // The press this release ends, if there was one on the canvas. Consumed
    // here either way, so a later release can never reuse it.
    const press = pressClientRef.current;
    pressClientRef.current = null;

    if (moveDrag) {
      // Block while snap-to-merge dialog is open
      if (snapMergeDialogPendingRef.current) {
        return;
      }
      // The drop is CLICK-gated (#409): a plain drag pans the map now, so only
      // a press that stayed put commits. Anything else leaves the ghost in
      // flight, still following the cursor, and lets MapLibre's dragPan own the
      // gesture - including a release that lands outside the canvas, which
      // never reaches this handler at all.
      //
      // `press === null` means the gesture did not start on the canvas (a press
      // on the floating chrome, or none at all) or has already travelled past
      // the threshold (handlePointerMove forgets it). Neither is a drop.
      //
      // The threshold is the one the custom pan claims on, so the two agree on
      // one boundary with one meaning: under CUSTOM_PAN_CLAIM_PX the gesture is
      // a click - which is also where MapLibre stops calling it one - at or
      // above it, it is a drag.
      //
      // Nothing here consults `ctrlKey`, deliberately: a still Ctrl+click drops
      // exactly like a plain one. Ctrl+click's other meaning is the selection
      // toggle, which `SurveyLayer.handleMapClick` already ignores under the
      // move tools, so there is nothing for the modifier to protect here.
      if (press === null) return;
      if (Math.hypot(e.clientX - press.x, e.clientY - press.y) >= CUSTOM_PAN_CLAIM_PX) return;
      handleMoveDragEnd();
    } else if (zoomBox.start) {
      handleZoomBoxEnd();
    }
  }, [moveDrag, zoomBox.start, handleMoveDragEnd, handleZoomBoxEnd]);

  // Pointer leave: keep all move drags alive so user can return the cursor
  const handlePointerLeave = useCallback((e: React.PointerEvent) => {
    if (moveDrag) return;
    handlePointerUp(e);
  }, [moveDrag, handlePointerUp]);

  // Handle placement mode click
  const handlePlacementClick = useCallback(
    (e: React.PointerEvent) => {
      if (!placementMode || !map.current || !mapContainer.current) return;

      // Left click confirms placement
      if (e.button === 0) {
        e.preventDefault();
        e.stopPropagation();
        noteMapInteraction();

        // On touch devices, pointerDown fires before any pointerMove, so
        // cursorPosition may be null. Compute it from the event to ensure
        // placement always has a valid position.
        const rect = mapContainer.current.getBoundingClientRect();
        const screenX = e.clientX - rect.left;
        const screenY = e.clientY - rect.top;
        const lngLat = map.current.unproject([screenX, screenY]);
        const meters = mapCoordsToMeters(lngLat.lng, lngLat.lat, gpsOrigin);
        updatePlacementCursor({ x: meters.x, y: meters.y, z: 0 });

        const snapped = checkPlacementSnap(screenX, screenY);
        updatePlacementSnap(snapped);

        handlePlacementConfirm();
      }
    },
    [placementMode, handlePlacementConfirm, gpsOrigin, updatePlacementCursor, updatePlacementSnap, checkPlacementSnap, noteMapInteraction]
  );

  // Handle placement mode right-click (cancel)
  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      if (placementMode) {
        e.preventDefault();
        e.stopPropagation();
        noteMapInteraction();
        handlePlacementCancel();
      }
    },
    [placementMode, handlePlacementCancel, noteMapInteraction]
  );

  // Determine pointerdown handler based on mode and tool
  // Only process events that originate from the map canvas (not UI controls or overlays)
  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      // Ignore clicks on overlay UI (buttons, indicators, MapLibre controls)
      // Gate on canvas element to exclude .maplibregl-control-container clicks
      const canvas = map.current?.getCanvas();
      if (!canvas || !canvas.contains(e.target as Node)) {
        return;
      }
      // Remember where the press landed, for the click gate in
      // `handlePointerUp`. Recorded for every canvas press rather than only
      // while carrying a ghost: a press cannot outlive its own release, so a
      // fresh press always overwrites, and there is no stale value for a later
      // gesture to inherit.
      pressClientRef.current = { x: e.clientX, y: e.clientY };
      if (placementMode) {
        handlePlacementClick(e);
      } else if (activeTool === 'zoom') {
        handleZoomBoxStart(e);
      }
    },
    [placementMode, activeTool, handlePlacementClick, handleZoomBoxStart]
  );

  // What the move-tool hint banner talks about (#409), or null when there is nothing to
  // hint at. An in-flight drag wins over the active tool: `moveDrag` outlives a mid-drag
  // tool switch, and it is what the drop instruction is about.
  const moveHintKind: 'station' | 'segment' | 'shot' | null =
    moveDrag?.type ??
    (activeTool === 'move_station'
      ? 'station'
      : activeTool === 'move_segment'
        ? 'segment'
        : activeTool === 'move_shot'
          ? 'shot'
          : null);

  // Pointer handlers always attached - they check e.ctrlKey internally to decide behavior
  return (
    <div
      className="relative w-full h-full"
      style={{ touchAction }}
      data-testid="map-canvas"
      data-map-loaded={mapLoaded ? 'true' : 'false'}
      data-active-tool={activeTool}
      data-placement-mode={placementMode ? 'true' : 'false'}
      /* (#24) Ctrl BELIEVED down, which the swallowed-release guard can get wrong.
         The cursor cannot stand in for it - the drag drives that too. */
      data-ctrl-held={ctrlHeld ? 'true' : 'false'}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerLeave={handlePointerLeave}
      onContextMenu={handleContextMenu}
    >
      <div ref={mapContainer} className="w-full h-full" data-testid="maplibre-container" />
      {webgl2Unavailable && <MapUnavailable testId="map-webgl2-unavailable" />}

      {/* Zoom box overlay */}
      {zoomBox.start && zoomBox.end && (
        <div
          className="absolute border-2 border-accent bg-accent-20 pointer-events-none"
          style={{
            left: Math.min(zoomBox.start.x, zoomBox.end.x),
            top: Math.min(zoomBox.start.y, zoomBox.end.y),
            width: Math.abs(zoomBox.end.x - zoomBox.start.x),
            height: Math.abs(zoomBox.end.y - zoomBox.start.y),
          }}
          data-testid="zoom-box-overlay"
        />
      )}

      {/* Grid overlay using MapLibre layer */}
      {mapLoaded && map.current && <GridOverlay map={map.current} gpsOrigin={gpsOrigin} />}

      {/* Depth / distance colour key + its colour-scale cog (distance wins the
          slot; both hidden while issue-severity owns the paint, #57). */}
      <MapColorLegends
        showDepth={effectiveDepthGradient && !showDistanceGradient}
        depthRange={depthRange}
        surveyData={activeSurveyData}
        showDistance={showDistanceGradient && !effectiveIssueSeverity}
        distanceField={distanceField}
      />

      {/* Gas reachable-frontier legend (#38) - gold→purple, 0 → turn budget. */}
      {showGasPlan && gasReachField && (
        <GasLegend field={gasReachField} units={gasPlanConfig.units} visible={showGasPlan} />
      )}
      {showGasPlan && gasOriginChosen && !gasReachField && (
        <div
          data-testid="gas-legend-loading"
          style={{
            position: 'absolute',
            bottom: 50,
            left: 10,
            background: 'var(--cc-text-halo-alt)',
            color: 'var(--cc-depth-text)',
            borderRadius: 4,
            padding: 8,
            zIndex: 1,
            fontSize: 11,
          }}
        >
          {t('map.gasLegend.computing')}
        </div>
      )}

      {/* Developer stats overlays (hamburger menu ▸ Debug) */}
      {(showDevStats || showMapStats || showZoomStats) && mapLoaded && map.current && (
        <MapDevStatsPanels
          map={map.current}
          showDevStats={showDevStats}
          showMapStats={showMapStats}
          showZoomStats={showZoomStats}
        />
      )}

      {/* Placement mode overlay */}
      {placementMode && placementCursor && (
        <PlacementOverlay
          sectionData={placementMode.sectionData}
          alignStationIndex={placementMode.alignStationIndex}
          cursorPosition={placementCursor}
          isSnapped={placementMode.snappedToStation !== null}
          pixelsPerMeter={pixelsPerMeter}
          additionalSections={placementMode.additionalSections}
          onCancel={handlePlacementCancel}
        />
      )}

      {/* Fit-to-view / exit-focus (#50) plus the read-only notices, laid out as
          one flex row so they can never overlap. Both rules for the closure notice
          - Survey only, and only when nothing else already explains the read-only
          state - live in `showsClosureAdjustedNotice`, next to `isMapReadOnly`. */}
      <MapTopLeftOverlay
        focused={focusedSectionId !== null}
        onFitOrExitFocus={() => (focusedSectionId !== null ? exitSectionFocus() : fitToBounds())}
        showSpeleoReadOnly={speleoReadOnly && !placementMode && appMode === 'survey'}
        showClosureReadOnly={showsClosureAdjustedNotice({
          geometryMode,
          appMode,
          placementMode: placementMode !== null,
          speleoReadOnly,
        })}
        // The solve still runs where the notice is hidden (Plan/Export force
        // closure-adjusted geometry), and it is the only progress signal there -
        // so the indicator is gated on the solve, not on the notice.
        showClosureComputing={closureAdjustedView && !placementMode && adjustedLoading}
      />

      {/* Placement mode indicator */}
      {placementMode && (
        <div
          className="absolute top-2 left-1/2 -translate-x-1/2 bg-panel border border-accent rounded px-3 py-2 text-sm"
          data-testid="placement-mode-indicator"
        >
          <span className="text-accent">{t('map.placement.title')}:</span>{' '}
          {t('map.placement.instructions')}
          {placementMode.snappedToStation && (
            <span className="ml-2 text-success">{t('map.placement.snapped')}</span>
          )}
        </div>
      )}

      {/* Merge tool indicator */}
      {activeTool === 'merge' && mergeStationA && !placementMode && (
        <div className="absolute top-2 left-1/2 -translate-x-1/2 bg-panel border border-accent rounded px-3 py-2 text-sm">
          <span className="text-accent">{t('map.merge.title')}:</span>{' '}
          {t('map.merge.instructions')}
        </div>
      )}

      {/* Measure tool indicator */}
      {activeTool === 'measure' && measureStart && !measureEnd && !placementMode && (
        <div className="absolute top-2 left-1/2 -translate-x-1/2 bg-panel border border-accent rounded px-3 py-2 text-sm">
          <span className="text-accent">{t('map.measure.title')}:</span>{' '}
          {t('map.measure.instructions')}
        </div>
      )}

      {/* Measurement result display (#26: direct + along-cave-line). Persists
          until a reset action (next click, Escape, tool switch, …). */}
      {measureEnd && measureDirect && !placementMode && (
        <div
          className="absolute top-2 left-1/2 -translate-x-1/2 bg-panel border border-border rounded px-3 py-2 text-sm"
          data-testid="measure-readout"
        >
          <span className="font-medium">{t('map.measure.direct')}:</span>{' '}
          <span data-testid="measure-direct">
            {formatFixed(measureDirect.distance, undefined, precision.length)}{t('common.units.meters')}
          </span>
          <span className="mx-2">|</span>
          <span className="font-medium">{t('map.measure.alongLine')}:</span>{' '}
          <span data-testid="measure-along-line">
            {measureStart?.kind === 'point' || measureEnd.kind === 'point'
              ? '-'
              : measureAlongLinePending
                ? '…'
                : measureAlongLine?.alongLine != null
                  ? `${formatFixed(measureAlongLine.alongLine, undefined, precision.length)}${t('common.units.meters')}`
                  : t('map.measure.noPath')}
          </span>
          <span className="mx-2">|</span>
          <span className="font-medium">{t('map.measure.depthDiff')}:</span>{' '}
          <span data-testid="measure-depth-diff">
            {formatFixed(measureDirect.depthDiff, undefined, precision.depth)}{t('common.units.meters')}
          </span>
        </div>
      )}

      {/* Move-tool hint (#409). The move tools are click-to-pick-up -> ghost follows the
          cursor -> click-to-drop; a press-drag pans the camera by design, so nothing
          about the interaction is discoverable without this banner. Same chrome and the
          same `!placementMode` mutual exclusion as the placement/merge/measure
          indicators above; it is the only one of them that can be up while a move tool
          is idle, hence `pointer-events-none` - it covers a strip of the map for as long
          as the tool is active, and a pick-up or drop click that lands on it has to
          reach the canvas. (`handlePointerDown` already ignores non-canvas targets, so
          without the pass-through such a click would be swallowed outright.)
          Not gated on `snapMergeDialogPendingRef`: that's a ref, so it wouldn't
          re-render the banner anyway, and the dialog it tracks is a modal overlay. */}
      {moveHintKind !== null && !placementMode && (
        <div
          className="absolute top-2 left-1/2 -translate-x-1/2 bg-panel border border-accent rounded px-3 py-2 text-sm pointer-events-none"
          data-testid="move-tool-hint"
        >
          <span className="text-accent">
            {moveHintKind === 'station'
              ? t('map.move.station')
              : moveHintKind === 'segment'
                ? t('map.move.segment')
                : t('map.move.shot')}
            :
          </span>{' '}
          {moveDrag
            ? t('map.move.dropInstructions')
            : moveHintKind === 'station'
              ? t('map.move.pickUpInstructions.station')
              : moveHintKind === 'segment'
                ? t('map.move.pickUpInstructions.segment')
                : t('map.move.pickUpInstructions.shot')}
        </div>
      )}

      {/* Comment tool overlay */}
      <CommentOverlay />

      {/* Move comment cursor tag - shows picked-up comment following cursor */}
      <MoveCommentCursorTag />

      {/* Hover sync - lightweight child to avoid re-rendering MapCanvas on every mousemove */}
      <MapHoverSync
        surveyLayerRef={surveyLayerRef}
        mapLoaded={mapLoaded}
        anchoredSectionIds={anchoredSectionIds}
        getConnectedSectionIds={getConnectedSectionIds}
        logMapInteractions={logMapInteractions}
      />
    </div>
  );
}

// Memoized: MapCanvas takes no props and drives itself from the stores, so it
// must NOT re-render just because a parent (SingleSystemView) re-renders - e.g.
// on every tick of a panel resize. Without this the whole (large) map component
// re-rendered ~60×/s during a drag, which is what made resizing feel heavy.
export default memo(MapCanvas);
