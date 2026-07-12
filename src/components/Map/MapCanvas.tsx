import { useEffect, useLayoutEffect, useRef, useCallback, useState, useMemo, memo, startTransition } from 'react';
import { useTranslation } from 'react-i18next';
import maplibregl from 'maplibre-gl';
import type { SectionId, ShotId, StationRef, DistanceField, Marker } from '../../types/project';
import type { GasReachField } from '../../types/ipc.generated';
import type { BatchCommand, ProjectWarning } from '../../types/commands';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore, isMapReadOnly, type Selection } from '../../store/uiStore';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { selectActiveStations, selectActiveSurveyData } from '../../store/geometrySelectors';
import { isMutatingTool } from '../../utils/toolUtils';
import { hasEligibleClosures } from '../../utils/closures';
import { useShallow } from 'zustand/react/shallow';
import {
  getEffectiveShowReferenceShots,
  getEffectiveShowShotData,
  getEffectiveShowStationComments,
  getEffectiveShowShotComments,
} from '../../store/displaySelectors';
import { getErrorMessage, invokeIpc } from '../../utils/ipc';
import { buildDistanceFeatureState } from '../../utils/distanceGradient';
import { buildGasReachFeatureState } from '../../utils/gasReach';
import { buildMeasuredPathIds, buildMeasuredStationIds, MEASURE_TINT } from '../../utils/measurePath';
import { getBounds, metersToMapCoords, mapCoordsToMeters } from '../../utils/coordinates';
import { registerTileProtocol, unregisterTileProtocol } from '../../utils/tileProtocol';
import { getTileConfig } from '../../utils/tileConfig';
import { useMapTools } from '../../hooks/useMapTools';
import SurveyLayer from './SurveyLayer';
import PlacementOverlay from './PlacementOverlay';
import GridOverlay from './GridOverlay';
import { DepthLegend } from './DepthLegend';
import { DistanceLegend } from './DistanceLegend';
import { GasLegend } from './GasLegend';
import { MapDevStats } from './MapDevStats';
import CommentOverlay from './CommentOverlay';
import MoveCommentCursorTag from './MoveCommentCursorTag';
import { installPanZoomDebug } from './panZoomDebug';
import { getMapThemeColors } from '../../utils/themeColors';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { FitToViewIcon, LockIcon } from '../common/icons';
import { useSpeleoStore, selectSpeleoReadOnly } from '../../store/speleoStore';
import { useNetworkStore } from '../../store/networkStore';
import { FEATURE_ID_STRIDE } from '../../utils/ids';
import { setSource } from '../../utils/telemetry';
import { debugLog } from '../../utils/debugLog';

// Merge preview marker (from the detail-panel slider) with the project's persisted
// markers. The preview replaces any existing marker for the same shot so the map
// shows live position updates while the form is open.
function effectiveMarkers(base: Marker[] | undefined, preview: Marker | null): Marker[] | undefined {
  if (!preview) return base;
  const filtered = (base ?? []).filter(
    (m) => !(m.section_id === preview.section_id && m.shot_id === preview.shot_id)
  );
  return [...filtered, preview];
}

// Tools that support temporary panning overrides (Ctrl+drag or middle-drag)
const CTRL_PAN_TOOLS = ['select', 'zoom', 'merge', 'measure', 'move_station', 'move_shot', 'move_segment', 'comment', 'move_comment'];

// Stable empty warnings array fed to the survey-layer rebuild effect when
// Issue-severity coloring is OFF. Module-level so its identity never changes
// across renders — a per-render [] would defeat the rebuild-gating memo below.
const EMPTY_WARNINGS: ProjectWarning[] = [];

interface BoxSelectState {
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

function MapCanvas() {
  const precision = useDecimalPrecision();
  const mapContainer = useRef<HTMLDivElement>(null);
  const map = useRef<maplibregl.Map | null>(null);
  const surveyLayerRef = useRef<SurveyLayer | null>(null);
  const originMarkerRef = useRef<maplibregl.Marker | null>(null);
  const gasOriginMarkerRef = useRef<maplibregl.Marker | null>(null);
  const gasTurnMarkersRef = useRef<maplibregl.Marker[]>([]);
  const gasZeroDatumMarkersRef = useRef<maplibregl.Marker[]>([]);
  const gasDropMarkersRef = useRef<maplibregl.Marker[]>([]);
  const gasFinalTurnMarkerRef = useRef<maplibregl.Marker | null>(null);
  // Keyboard-picking a turn marker nulls reachField (setEndpoint), which tears down
  // every turn marker incl. the focused one; when the recomputed field rebuilds them,
  // focus would be stranded on <body>. Remember the picked ref so the rebuild can
  // restore focus to its new marker element — the a11y feature would otherwise lose
  // the AT user's place on every pick (#243).
  const pendingTurnFocusRef = useRef<StationRef | null>(null);
  // Same story for a keyboard-nudged / reset ▼ drop marker (#246): the recompute rebuilds every
  // drop marker, so remember the drop INDEX (stable across the move - the ref itself shifts) to
  // restore focus to its rebuilt element.
  const pendingDropFocusRef = useRef<number | null>(null);
  const scaleControlRef = useRef<maplibregl.ScaleControl | null>(null);
  const scaleControlAddedRef = useRef<boolean>(false);
  const [boxSelect, setBoxSelect] = useState<BoxSelectState>({ start: null, end: null });
  const [zoomBox, setZoomBox] = useState<BoxSelectState>({ start: null, end: null });
  const [moveDrag, setMoveDrag] = useState<MoveDragState | null>(null);
  const [mapLoaded, setMapLoaded] = useState(false);
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
  // Ref mirror of snappedStation — used for same-value comparison in the snap hot path
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
  // Token to invalidate stale getConnectedSectionIds results (incremented on each drag start,
  // checked in .then()/.catch() to discard late async results after mouseup)
  const segmentDragTokenRef = useRef(0);
  // --- Drag hot-path refs (bypass React state to avoid re-render per mousemove) ---
  // Current drag position (updated imperatively on every mousemove)
  const moveDragLngLatRef = useRef<{ lng: number; lat: number } | null>(null);
  // Last cursor position (client coords) during a move-drag — lets us re-apply the
  // preview on map zoom (when no pointer event fires) so it tracks across zoom (#79).
  const lastCursorClientRef = useRef<{ clientX: number; clientY: number } | null>(null);
  // Connected section IDs being dragged (used by rAF callback for segment preview)
  const dragConnectedSectionIdsRef = useRef<Set<number>>(new Set());
  // Origin station that initiated the segment drag (for snap-to-merge)
  const dragOriginStationRef = useRef<StationRef | null>(null);
  // Current snap target during drag (ref, not state — updated per mousemove)
  const dragSnapRef = useRef<StationRef | null>(null);
  // Last raw (pre-snap) pixel offset — used to restore preview if snap-merge is cancelled
  const lastRawDragOffsetRef = useRef<{ dx: number; dy: number } | null>(null);
  // True while snap-to-merge dialog is open — blocks mouseup from ending drag
  const snapMergeDialogPendingRef = useRef(false);
  // Co-located station refs for station drag (includes merged/closured stations)
  const stationDragColocatedRefsRef = useRef<StationRef[]>([]);
  const { t } = useTranslation();

  const { project, stations, surveyData, stationsAdjusted, surveyDataAdjusted, adjustedLoading, loadAdjustedGeometry, config, warnings, moveStation, moveSegment, getConnectedSectionIds, getAnchoredSectionIds, executeBatch } = useProjectStore();
  const logMapInteractions = config?.debug?.map?.log_hit_detection ?? false;

  const previewMarker = useUIStore((s) => s.previewMarker);

  // Geometry display mode (measured vs read-only closure-adjusted).
  const geometryMode = useUIStore((s) => s.geometryMode);
  // T2b: a remote SpeleoDB project opened without the write lock shows a
  // top-left read-only badge, mirroring the closure-adjusted view-only badge.
  const speleoReadOnly = useSpeleoStore(selectSpeleoReadOnly);
  const setGeometryMode = useUIStore((s) => s.setGeometryMode);
  const setActiveTool = useUIStore((s) => s.setActiveTool);

  // #50: section-focus mode — show an exit-focus control at the map's top-left
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
    // shape — accepted regression cost in that corner case.
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

  // Performance: exclude hover values (hoveredStation, hoveredShot, hoveredSectionId)
  // from the store subscription. These change ~60 times/sec during mouse movement
  // and would cause this 2700-line component to re-render every frame.
  // Hover sync is handled by <MapHoverSync> child component instead.
  const {
    mapZoom,
    setMapCenter,
    setMapZoom,
    activeTool,
    showSatellite,
    showShotData,
    showSectionNames,
    showStationLabels,
    showStationComments,
    showShotComments,
    showStations,
    showShots,
    showReferenceShots,
    showDepthGradient,
    showDistanceGradient,
    appMode,
    showIssueSeverity,
    distanceGradientOrigin,
    distanceField,
    setDistanceField,
    setDistanceGradientOrigin,
    showDevStats,
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
    addToSelection,
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
  } = useUIStore(
    // measureSnapShot is excluded: the live shot-snap `t` is rewritten on every
    // mousemove over a shot, and the cursor dot is driven imperatively via refs,
    // so subscribing would re-render this large component ~60×/s (#26). The
    // leave-tool cleanup below reads it via getState() instead.
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    useShallow(({ hoveredStation, hoveredShot, hoveredSectionId, measureSnapShot, ...rest }) => rest)
  );

  // The gas overlay belongs to the Plan workspace mode (the former showGasPlan
  // sub-mode). Derived so the existing showGasPlan-gated effects below are unchanged.
  const showGasPlan = appMode === 'plan';

  // Gas & route-planning sub-mode inputs (#38) — a separate store so editing the
  // diver/cylinders/config doesn't re-render via the big uiStore subscription.
  const {
    origin: gasOrigin,
    endpoint: gasEndpoint,
    diver: gasDiver,
    cylinders: gasCylinders,
    gasConfig: gasPlanConfig,
    reachField: gasReachField,
    stageDropOverrides: gasStageDropOverrides,
    setReachField: setGasReachField,
    setOrigin: setGasOrigin,
    setStageDropOverride: setGasStageDropOverride,
  } = useGasPlanStore(
    useShallow((s) => ({
      origin: s.origin,
      endpoint: s.endpoint,
      diver: s.diver,
      cylinders: s.cylinders,
      gasConfig: s.gasConfig,
      reachField: s.reachField,
      stageDropOverrides: s.stageDropOverrides,
      setReachField: s.setReachField,
      setOrigin: s.setOrigin,
      setStageDropOverride: s.setStageDropOverride,
    }))
  );

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
  // Gate the survey-layer rebuild on issue-severity coloring. `warnings` changes
  // identity on every refreshWarnings (each edit/resolve/ignore), but warnings
  // only affect colors when severity coloring is effectively ON — feed the rebuild
  // a stable shared [] when OFF so warning-only refreshes don't rebuild the GeoJSON.
  const severityWarnings = useMemo(
    () => (effectiveIssueSeverity ? warnings : EMPTY_WARNINGS),
    [effectiveIssueSeverity, warnings]
  );

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
  // zero-datum station via merged_with and aligned closures_with links.
  const immovableStationKeys = useMemo(() => {
    if (activeTool !== 'move_station' && activeTool !== 'move_shot') return new Set<string>();
    const stationByKey = new Map(
      stations.map(s => [`${s.station_ref.section_id}:${s.station_ref.station_index}`, s])
    );
    const immovable = new Set<string>();
    const queue: string[] = [];
    for (const s of stations) {
      if (s.is_zero_datum) {
        queue.push(`${s.station_ref.section_id}:${s.station_ref.station_index}`);
      }
    }
    while (queue.length > 0) {
      const key = queue.pop()!;
      if (immovable.has(key)) continue;
      immovable.add(key);
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
    return immovable;
  }, [activeTool, stations, alignedClosureIds]);

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
  // indicator tracks the cursor at frame rate — same strategy as hover detection.
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
      // within range — stations win when both are close.
      const nearestShot =
        activeTool === 'measure' && !nearestStation
          ? surveyLayerRef.current.queryNearestShotPoint(screenX, screenY, snapConfig.threshold)
          : null;

      // Station snap — compare via ref (no React state dependency in the hot path).
      const prev = snappedStationRef.current;
      if (nearestStation) {
        if (
          !prev ||
          prev.section_id !== nearestStation.section_id ||
          prev.station_index !== nearestStation.station_index
        ) {
          snappedStationRef.current = nearestStation;
          // Direct visual update — no React re-render needed
          surveyLayerRef.current.setSnappedStation(nearestStation);
          // Deferred store update for click handling — startTransition prevents
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
    // The measure shot-snap (#26) only exists for the measure tool — drop its
    // marker + state when leaving it so a stale cyan dot can't linger.
    if (activeTool !== 'measure' && (shotSnapMarkerRef.current || measureSnapShotRef.current)) {
      shotSnapMarkerRef.current?.remove();
      shotSnapMarkerRef.current = null;
      measureSnapShotRef.current = null;
      // measureSnapShot isn't subscribed (see useShallow above) — read it live.
      if (useUIStore.getState().measureSnapShot) setMeasureSnapShot(null);
    }
  }, [activeTool, snappedStation, setSnappedStation, setMeasureSnapShot]);

  // Tear down the shot-snap marker + ref whenever the store snap clears, even while
  // the measure tool stays active — a reset that keeps the tool (Escape, dialog,
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

  // Hiding Shots disables shot interaction; hiding only Reference Shots disables it
  // for reference legs. Drop a live measure shot-snap in either case (when the snap
  // is on a now-hidden reference leg), so the cyan marker doesn't linger on hidden
  // geometry — the null-ness effect above then removes the marker + ref (#26, Codex).
  useEffect(() => {
    const snap = useUIStore.getState().measureSnapShot;
    if (!snap) return;
    const onHiddenReferenceLeg =
      !showReferenceShots &&
      (project?.sections
        .find((s) => s.id === snap.section_id)
        ?.shots.find((sh) => sh.id === snap.shot_id)?.is_reference ?? false);
    if (!showShots || onHiddenReferenceLeg) setMeasureSnapShot(null);
  }, [showShots, showReferenceShots, project?.sections, setMeasureSnapShot]);

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
      // zoom in — anchoring on the feature's world coord makes IT (not the
      // imprecise click point) track the cursor through zoom (#79). A station grab
      // snaps to the station; a shot grab snaps to the nearest point on the shot.
      // NOTE: this baseline reads `activeStations` (geometry-mode-aware), while the
      // per-frame handleMoveDragMove and the commit read raw `stations`. They diverge
      // only in `closureAdjusted` mode — but move tools are mutating, so the tool is
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
        // Begin drag preview — returns co-located stations (merged/closured at same coords)
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
        // Check if section is anchored — silent no-op if so
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
          // Begin drag preview — creates temp layers with *-translate paint property
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
        // Begin drag preview seeded from both shot endpoints — returns all co-located stations
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

  // Combine with hidden feature callbacks for toast feedback
  const toolCallbacks = useMemo(() => ({
    ...baseToolCallbacks,
    onHiddenStationClick: () => {
      showToast("Enable 'Stations' in Display menu to interact with station markers", 'stations-hidden');
    },
    onHiddenShotClick: () => {
      showToast("Enable 'Shots' in Display menu to interact with shot lines", 'shots-hidden');
    },
    onMarkerClick: (sectionId: number, shotId: number) => {
      const store = useUIStore.getState();
      store.setSelection({ type: 'shot', sectionId, shotId });
      store.setPendingMarkerOpen({ sectionId, shotId });
    },
  }), [baseToolCallbacks, showToast]);

  // Box select handlers
  const handleBoxSelectStart = useCallback(
    (e: React.PointerEvent) => {
      if (activeTool !== 'select') return;
      if (!mapContainer.current) return;

      // Only start box select on left click and not on a feature
      if (e.button !== 0) return;

      // Don't start box select when Ctrl is held (Ctrl+drag is for panning)
      // Use e.ctrlKey directly instead of state to avoid timing issues
      if (e.ctrlKey) return;

      const rect = mapContainer.current.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;

      setBoxSelect({ start: { x, y }, end: { x, y } });
    },
    [activeTool]
  );

  const handleBoxSelectMove = useCallback(
    (e: React.PointerEvent) => {
      if (!boxSelect.start || !mapContainer.current) return;

      const rect = mapContainer.current.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;

      setBoxSelect((prev) => ({ ...prev, end: { x, y } }));
    },
    [boxSelect.start]
  );

  const handleBoxSelectEnd = useCallback(() => {
    if (!boxSelect.start || !boxSelect.end || !map.current) {
      setBoxSelect({ start: null, end: null });
      return;
    }

    // Calculate selection box bounds in map coordinates
    const minX = Math.min(boxSelect.start.x, boxSelect.end.x);
    const maxX = Math.max(boxSelect.start.x, boxSelect.end.x);
    const minY = Math.min(boxSelect.start.y, boxSelect.end.y);
    const maxY = Math.max(boxSelect.start.y, boxSelect.end.y);

    // Only process if box is large enough (not just a click)
    if (maxX - minX > 5 && maxY - minY > 5) {
      // Query stations in the box using hitbox layer (larger clickable area)
      const features = map.current.queryRenderedFeatures(
        [[minX, minY], [maxX, maxY]],
        { layers: ['survey-stations-hitbox'] }
      );

      if (features.length > 0) {
        // Clear existing selection if not holding shift
        clearSelection();
        setSource('map');

        // Add all stations in box to selection
        features.forEach((feature) => {
          if (feature.properties) {
            const { sectionId, stationIndex } = feature.properties;
            const sel: Selection = {
              type: 'station',
              stationRef: {
                section_id: sectionId as number,
                station_index: stationIndex as number,
              },
            };
            addToSelection(sel);
          }
        });
      }
    }

    setBoxSelect({ start: null, end: null });
  }, [boxSelect, clearSelection, addToSelection]);

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

  // Move drag handlers — hot path bypasses React state for performance.
  // Position is stored in a ref (no re-render), and segment preview is updated
  // synchronously since setPaintProperty is essentially free (shader uniform).
  const handleMoveDragMove = useCallback(
    // skipSnapScan: the per-frame map-'move' re-apply (zoom/pan) passes true to skip the
    // O(all-stations) snap SCAN — the cursor's world position is unchanged so the snap
    // target can't change. The cheap snap RE-ALIGN (for an already-held target) and the
    // #79 re-projection still run, so the preview tracks and stays snapped across zoom.
    (clientX: number, clientY: number, skipSnapScan = false) => {
      // While the snap-merge dialog is up the drag is paused (moveDrag is held alive
      // so Cancel can resume it) — don't let the map-'move' listener re-apply the
      // preview and clobber the held position. Mirrors handleMoveDragStart/End, which
      // both already guard on this; this is the only drag-move entry that didn't.
      if (snapMergeDialogPendingRef.current) return;
      if (!moveDrag || !map.current || !mapContainer.current) return;

      const rect = mapContainer.current.getBoundingClientRect();
      const x = clientX - rect.left;
      const y = clientY - rect.top;

      // Convert screen coords to map coords
      const lngLat = map.current.unproject([x, y]);

      // Store in ref (no React state update — avoids re-render per mousemove)
      moveDragLngLatRef.current = { lng: lngLat.lng, lat: lngLat.lat };
      lastCursorClientRef.current = { clientX, clientY };

      // Re-project the grab's WORLD point at the CURRENT zoom (NOT a fixed grab-screen
      // baseline — that drifts when the map re-projects on zoom mid-drag, #79). dx/dy
      // is then the cursor offset from where the grabbed point renders right now.
      const grabScreen = map.current.project([moveDrag.startLngLat.lng, moveDrag.startLngLat.lat]);
      const dx = x - grabScreen.x;
      const dy = y - grabScreen.y;
      lastRawDragOffsetRef.current = { dx, dy };

      // Station drag preview — translate station circles/labels + rebuild shot geometries
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

      // Shot drag preview — same pipeline as station drag (both endpoints + co-located stations move)
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
  // programmatic pan) so the grabbed feature stays glued to the cursor — a
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

      showToast('Checking merge compatibility...', 'snap-merge-check');
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
          showToast(result.reason ?? 'Merge not possible.', 'snap-merge-blocked');
          moveCallbacks?.onCancel?.();
        }
      } catch (e) {
        showToast(getErrorMessage(e), 'snap-merge-error');
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
      // Early bail — clean up everything
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
        // — these keep the drag alive after Cancel.
        promptSnapMerge(originStationRef, snapTarget, {
          preCommands: [segmentMoveCmd],
          onBeforeConfirm: async () => {
            // UI cleanup only — backend move is batched with merge/closure by the dialog
            snapMergeDialogPendingRef.current = false;
            dragOriginStationRef.current = null;
            lastRawDragOffsetRef.current = null;
            surveyLayerRef.current?.onNextUpdate(() => {
              surveyLayerRef.current?.endDragPreview();
            });
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

        // Don't clear moveDrag or dragOriginStationRef —
        // these keep the drag alive after Cancel.
        promptSnapMerge(originStationRef, snapTarget, {
          preCommands: moveCommands,
          onBeforeConfirm: async () => {
            // UI cleanup only — backend move is batched with merge/closure by the dialog
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
        // successful refresh — which never comes — so the committed preview would
        // leave the dragged features hidden ("shots vanish"). Restore them now.
        if (!res.success) surveyLayerRef.current?.endDragPreview();
        setMoveDrag(null);
        return;
      } else if (moveDrag.type === 'segment' && moveDrag.sectionId !== undefined) {
        // Non-snap segment move
        surveyLayerRef.current?.onNextUpdate(() => {
          surveyLayerRef.current?.endDragPreview();
        });
        const res = await moveSegment(moveDrag.sectionId, offset);
        // Rejected move (read-only gate): restore now — see the station branch.
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

    // Calculate initial center from stations (in meters)
    const coords = stations.map((s) => s.coordinates);
    const bounds = getBounds(coords);
    const centerX = (bounds.minX + bounds.maxX) / 2;
    const centerY = (bounds.minY + bounds.maxY) / 2;
    // Transform to map coordinates
    const mapCenter = metersToMapCoords(centerX, centerY, gpsOrigin);

    map.current = new maplibregl.Map({
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
      zoom: mapZoom,
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
    // Disable box zoom (Ctrl+drag draws selection box by default, we want Ctrl+drag to pan)
    map.current.boxZoom.disable();

    // Add navigation controls
    map.current.addControl(
      new maplibregl.NavigationControl({ showCompass: false }),
      'top-right'
    );

    // Create the scale control instance (always shown; added once on map load).
    scaleControlRef.current = new maplibregl.ScaleControl({ maxWidth: 100, unit: 'metric' });

    // Track map movement
    map.current.on('move', () => {
      if (!map.current) return;
      const center = map.current.getCenter();
      setMapCenter([center.lng, center.lat]);
    });

    map.current.on('zoom', () => {
      if (!map.current) return;
      setMapZoom(map.current.getZoom());
      // Calculate pixels per meter at current zoom
      // Use map center to project 1 meter difference
      const center = map.current.getCenter();
      const centerScreen = map.current.project(center);
      // Point 1 meter north in map coordinates (1 meter = 1/111320 degrees)
      const northPoint = map.current.project([center.lng, center.lat + 1 / 111320]);
      const ppm = Math.abs(centerScreen.y - northPoint.y);
      setPixelsPerMeter(ppm > 0 ? ppm : 1);
    });

    // Initialize survey layer
    map.current.on('load', () => {
      if (!map.current) return;
      surveyLayerRef.current = new SurveyLayer(map.current, toolCallbacks);
      // Set initial tool immediately after creating SurveyLayer
      surveyLayerRef.current.setActiveTool(useUIStore.getState().activeTool);
      setMapLoaded(true);
      // Initialize pixels per meter
      const center = map.current.getCenter();
      const centerScreen = map.current.project(center);
      const northPoint = map.current.project([center.lng, center.lat + 1 / 111320]);
      const ppm = Math.abs(centerScreen.y - northPoint.y);
      setPixelsPerMeter(ppm > 0 ? ppm : 1);
    });

    return () => {
      // [perf] Time the MapCanvas teardown so we can isolate map.remove()
      // cost vs React unmount cost on the back-button freeze. Surfaced in
      // the MemoryProbe panel.
      const t0 = performance.now();
      if (snapRafId.current !== null) {
        cancelAnimationFrame(snapRafId.current);
        snapRafId.current = null;
      }
      // map.remove() tears down all layers, sources, the worker and the
      // canvas element atomically — calling surveyLayer.destroy() first
      // duplicates that work serially while the worker still holds a large
      // FeatureCollection, which blocks the main thread for seconds on a
      // 75k-shot survey (the "back-button is frozen" symptom). We rely on
      // the pending-update microtask in SurveyLayer.flushUpdate guarding
      // against `hasLiveStyle === false` so it bails harmlessly after
      // map.remove() runs.
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

  // Register custom tile protocol for offline caching
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

  // Expose the active meters transform for E2E commit-offset assertions — lets a test
  // predict a committed move offset with the SAME mapCoordsToMeters + gpsOrigin the
  // component uses (the no-GPS linear fallback vs the ENU transform are not equivalent).
  useEffect(() => {
    (window as unknown as { __stalagmight_to_meters__?: (lng: number, lat: number) => { x: number; y: number } })
      .__stalagmight_to_meters__ = (lng, lat) => mapCoordsToMeters(lng, lat, gpsOrigin);
    return () => {
      delete (window as unknown as { __stalagmight_to_meters__?: unknown }).__stalagmight_to_meters__;
    };
  }, [gpsOrigin]);

  // Update survey layer when data changes or map loads.
  // useLayoutEffect ensures setData() runs before the browser paints — critical
  // for drag commit where endDragPreview() must fire in the same frame as setData().
  useLayoutEffect(() => {
    if (surveyLayerRef.current && project) {
      surveyLayerRef.current.update(
        project.sections,
        activeStations,
        // Closure overlays are drawn from the active coordinates, so an adjusted
        // (closed) loop collapses to a zero-length, invisible line, while a loop
        // that wasn't closed -- an ineligible/deferred closure, or a component whose
        // solve fell back to measured -- stays visible. Pass closures in both modes
        // rather than hiding them wholesale, so the adjusted view doesn't look
        // resolved when some loops are still open.
        project.loop_closures,
        gpsOrigin,
        effectiveDepthGradient,
        depthRange,
        precision,
        effectiveIssueSeverity,
        severityWarnings,
        effectiveMarkers(project.markers, previewMarker),
      );
    }
  }, [project, activeStations, mapLoaded, gpsOrigin, effectiveDepthGradient, depthRange, precision, geometryMode, surveyDataAdjusted, effectiveIssueSeverity, severityWarnings, previewMarker]);

  // (The Warnings-tab auto-highlight is now derived via `effectiveIssueSeverity`
  // above — no snapshot/restore effect needed, so nothing to lose on leave/unmount.)

  // On a project switch (created_at changes on switch, stable across edits AND
  // saves), reset the distance origin to the entrance default and drop the stale
  // field — otherwise the previous project's distances paint the new project gray
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
  // a distinct file that happens to share created_at — e.g. a duplicated project —
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
  useEffect(() => {
    clearMeasure();
  }, [project?.modified_at, clearMeasure]);

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
  // display geometryMode affects them — no need to refetch on either. `from` null
  // = the entrance (zero-datum) default.
  useEffect(() => {
    if (!showDistanceGradient) return undefined;
    let cancelled = false;
    invokeIpc<DistanceField>(
      'get_station_distances_from',
      { from: distanceGradientOrigin ?? null },
      { purpose: 'distance gradient field', fields: { origin: distanceGradientOrigin ?? 'entrance' } }
    )
      .then((field) => { if (!cancelled) setDistanceField(field); })
      .catch((e) => console.warn('[distance-gradient] field fetch failed', e));
    return () => { cancelled = true; };
  }, [showDistanceGradient, distanceGradientOrigin, project?.modified_at, setDistanceField]);

  // Gas reachable-frontier overlay (#38): fetch the reachability field when the
  // sub-mode is on and any input (origin / diver / cylinders / config) or the
  // project data changes. Like distances it uses MEASURED geometry, so the display
  // geometryMode / closure config don't affect it. No debounce: the compute is
  // ms-fast and the inputs are discrete (number fields / presets), not continuous
  // sliders. `origin` null = the entrance (zero-datum) default; the command
  // returns null when no project is open (Ok(None)) or throws on invalid input.
  useEffect(() => {
    if (!showGasPlan) return undefined;
    let cancelled = false;
    invokeIpc<GasReachField | null>(
      'compute_gas_reach_field',
      {
        origin: gasOrigin ?? null,
        endpoint: gasEndpoint ?? null,
        diver: gasDiver,
        cylinders: gasCylinders,
        gasConfig: gasPlanConfig,
        // Per-stage drop overrides (#246): the station each ▼ marker was dragged to. Empty when
        // nothing is dragged; the backend clamps each pin before it can move the budget.
        stageDropOverrides: gasStageDropOverrides,
      },
      { purpose: 'gas reachable-frontier field', fields: { origin: gasOrigin ?? 'entrance' } }
    )
      .then((field) => { if (!cancelled) setGasReachField(field); })
      .catch((e) => console.warn('[gas-plan] field fetch failed', e));
    return () => { cancelled = true; };
  }, [showGasPlan, gasOrigin, gasEndpoint, gasDiver, gasCylinders, gasPlanConfig, gasStageDropOverrides, project?.modified_at, setGasReachField]);

  // Apply the fetched field to the layer (feature-state + paint swap). Separate
  // from the rebuild path so re-rooting never triggers a feature rebuild.
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    if (showDistanceGradient && !effectiveIssueSeverity && distanceField && project) {
      const state = buildDistanceFeatureState(project.sections, distanceField);
      surveyLayerRef.current.setDistanceGradient(true, state);
    } else {
      // Severity coloring (incl. the Warnings-tab auto-highlight) takes precedence.
      surveyLayerRef.current.setDistanceGradient(false, null);
    }
  }, [showDistanceGradient, effectiveIssueSeverity, distanceField, project, mapLoaded, surveyLayerRef]);

  // Apply the gas field to the layer (feature-state + paint swap). Separate from
  // the fetch so re-rooting / re-computing never triggers a feature rebuild.
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    if (showGasPlan && gasReachField && project) {
      const state = buildGasReachFeatureState(project.sections, gasReachField);
      surveyLayerRef.current.setGasReach(true, state);
    } else {
      surveyLayerRef.current.setGasReach(false, null);
    }
  }, [showGasPlan, gasReachField, project, mapLoaded, surveyLayerRef]);

  // Highlight the picked route over the survey (#233): full-opacity swum legs + faded jumps (#243).
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    if (showGasPlan && gasReachField && project) {
      surveyLayerRef.current.setGasRoute(gasReachField.route);
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
  // origin — the entrance default is the zero datum(s), already starred.
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
  // reachable ramp). Only for a *picked* origin — the entrance default is the
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

  // Gas turn-frontier markers (#38): a red dot on each reachable station that
  // borders an unreachable one — "this is where you turn", and the candidate end points.
  // Right-click one to open the Plan menu (Set as endpoint / origin) — these stay visible
  // even when survey stations are hidden, so they're the reliable route-pick target.
  // Rebuilt whenever the field changes; bounded by the frontier size (a handful of leads).
  useEffect(() => {
    for (const m of gasTurnMarkersRef.current) m.remove();
    gasTurnMarkersRef.current = [];
    if (!map.current || !mapLoaded || !showGasPlan || !gasReachField) return undefined;
    const mapInst = map.current;
    for (const ref of gasReachField.turn_frontier) {
      const st = activeStations.find(
        (s) =>
          s.station_ref.section_id === ref.section_id &&
          s.station_ref.station_index === ref.station_index
      );
      if (!st) continue;
      const lngLat = metersToMapCoords(
        st.coordinates.x,
        st.coordinates.y,
        gpsOrigin
      ) as [number, number];
      const el = document.createElement('div');
      el.setAttribute('data-testid', 'gas-turn-marker');
      el.setAttribute('role', 'button');
      // Keyboard-operable like the zero-datum star: focusable + Enter/Space, so
      // keyboard / AT users can pick a route endpoint too (#243).
      el.setAttribute('tabindex', '0');
      el.setAttribute('aria-label', t('gasPanel.map.turnMarkerAria'));
      el.title = t('gasPanel.map.turnMarkerTooltip');
      el.style.cssText =
        'width:14px;height:14px;border-radius:50%;border:2px solid #ffffff;' +
        'background:#c0392b;box-shadow:0 0 3px rgba(0,0,0,0.7);cursor:pointer;';
      // Left-click a candidate end point → select it as the route endpoint (#243).
      const pick = () => useGasPlanStore.getState().setEndpoint(ref);
      el.addEventListener('click', (e) => {
        e.stopPropagation();
        pick();
      });
      el.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          // Keyboard pick: setEndpoint will rebuild every marker, so record which
          // one to re-focus once the recomputed field lands (see restore below).
          pendingTurnFocusRef.current = ref;
          pick();
        }
      });
      // Right-click a candidate end point → the Plan context menu for that station.
      el.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        e.stopPropagation();
        useUIStore.getState().openDialog('context-menu', {
          target: { type: 'station', ref },
          position: { x: e.clientX, y: e.clientY },
        });
      });
      gasTurnMarkersRef.current.push(
        new maplibregl.Marker({ element: el }).setLngLat(lngLat).addTo(mapInst)
      );
      // Restore keyboard focus to the marker the AT user just activated: after the
      // pick's recompute rebuilt the frontier, the picked ref is still a turn
      // candidate (reachability is independent of the chosen endpoint), so its new
      // element gets focus and the user keeps their place.
      if (
        pendingTurnFocusRef.current &&
        pendingTurnFocusRef.current.section_id === ref.section_id &&
        pendingTurnFocusRef.current.station_index === ref.station_index
      ) {
        el.focus();
        pendingTurnFocusRef.current = null;
      }
    }
    return () => {
      for (const m of gasTurnMarkersRef.current) m.remove();
      gasTurnMarkersRef.current = [];
    };
  }, [showGasPlan, gasReachField, activeStations, gpsOrigin, mapLoaded, t]);

  // Zero-datum star markers (#38): a black star (white outline) on each zero-datum
  // station while planning. The gas solve's "Entrance" default already starts from the
  // zero datum(s); this marks them so the user can SEE where they are and click one to
  // pick it explicitly as the origin. Plan mode only; bounded by the datum count (≈1).
  useEffect(() => {
    for (const m of gasZeroDatumMarkersRef.current) m.remove();
    gasZeroDatumMarkersRef.current = [];
    if (!map.current || !mapLoaded || !showGasPlan) return undefined;
    for (const st of activeStations) {
      if (!st.is_zero_datum) continue;
      const lngLat = metersToMapCoords(st.coordinates.x, st.coordinates.y, gpsOrigin) as [
        number,
        number,
      ];
      const el = document.createElement('div');
      el.setAttribute('data-testid', 'gas-zero-datum-marker');
      el.setAttribute('role', 'button');
      // Keyboard-operable, not just clickable (it carries role="button"): focusable +
      // Enter/Space activation, so keyboard / AT users can pick the datum origin too.
      el.setAttribute('tabindex', '0');
      el.setAttribute('aria-label', t('gasPanel.origin.datumAria'));
      el.title = t('gasPanel.origin.datumTooltip');
      el.style.cssText = 'cursor:pointer;line-height:0;filter:drop-shadow(0 0 1px rgba(0,0,0,0.6));';
      el.innerHTML =
        '<svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">' +
        '<path d="M12 17.27L18.18 21l-1.64-7.03L22 9.24l-7.19-.61L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21z" ' +
        'fill="#000000" stroke="#ffffff" stroke-width="1.5" stroke-linejoin="round"/></svg>';
      const ref = st.station_ref;
      const pick = () => setGasOrigin(ref);
      el.addEventListener('click', (e) => {
        e.stopPropagation();
        pick();
      });
      el.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          pick();
        }
      });
      gasZeroDatumMarkersRef.current.push(
        new maplibregl.Marker({ element: el }).setLngLat(lngLat).addTo(map.current)
      );
    }
    return () => {
      for (const m of gasZeroDatumMarkersRef.current) m.remove();
      gasZeroDatumMarkersRef.current = [];
    };
  }, [showGasPlan, activeStations, gpsOrigin, mapLoaded, setGasOrigin, t]);

  // Staged-strategy route markers (#233): a ▼ drop marker where each stage is dropped along the
  // deepest-penetration route, plus a distinct final-turn marker where the back-gas leg runs out.
  // The backend leaves `stage_drop_refs`/`final_turn_ref` empty for All-thirds, so these only
  // appear for the Half-Minus / Free strategies. Plan mode only; bounded by the stage count.
  // Each ▼ is user-movable (#246): drag it (or arrow-key nudge it) to a route station and the
  // backend recomputes the whole plan; it turns red when dragged over-reserve; Delete/dbl-click
  // resets it to the strategy-computed spot.
  useEffect(() => {
    for (const m of gasDropMarkersRef.current) m.remove();
    gasDropMarkersRef.current = [];
    gasFinalTurnMarkerRef.current?.remove();
    gasFinalTurnMarkerRef.current = null;
    if (!map.current || !mapLoaded || !showGasPlan || !gasReachField) return undefined;
    const mapInst = map.current;
    const lngLatFor = (ref: { section_id: number; station_index: number }) => {
      const st = activeStations.find(
        (s) =>
          s.station_ref.section_id === ref.section_id &&
          s.station_ref.station_index === ref.station_index
      );
      return st
        ? (metersToMapCoords(st.coordinates.x, st.coordinates.y, gpsOrigin) as [number, number])
        : null;
    };

    // Ordered route stations (origin → turn) for snapping a dragged drop + the keyboard nudge.
    const firstLeg = gasReachField.route[0];
    const routeRefs: StationRef[] = firstLeg
      ? [firstLeg.from, ...gasReachField.route.map((l) => l.to)]
      : [];
    // Snap a dropped position to the nearest route station in SCREEN space. Project each station
    // FRESH at drag-end (not from a snapshot captured when the effect ran) so a pan/zoom between
    // build and drag can't snap to the wrong station; screen space keeps it accurate at any zoom.
    const snapToRoute = (lngLat: maplibregl.LngLat): StationRef | null => {
      const p = mapInst.project(lngLat);
      let best: StationRef | null = null;
      let bestD = Infinity;
      for (const r of routeRefs) {
        const ll = lngLatFor(r);
        if (!ll) continue;
        const sp = mapInst.project(ll);
        const d = (sp.x - p.x) ** 2 + (sp.y - p.y) ** 2;
        if (d < bestD) {
          bestD = d;
          best = r;
        }
      }
      return best;
    };

    gasReachField.stage_drop_refs.forEach((ref, i) => {
      const lngLat = lngLatFor(ref);
      if (!lngLat) return;
      const overReserve = gasReachField.stage_drop_over_reserve[i] === true;
      // Read the override fresh from the store: this effect rebuilds on the recomputed FIELD (not on
      // the override write, see the deps note), so the store is the current source at rebuild time.
      const overridden = useGasPlanStore.getState().stageDropOverrides[i] != null;
      const el = document.createElement('div');
      el.setAttribute('data-testid', 'gas-drop-marker');
      el.setAttribute('data-drop-index', String(i));
      el.setAttribute('data-over-reserve', String(overReserve));
      el.setAttribute('data-overridden', String(overridden));
      // A slider along the route: draggable with the mouse, nudged one station with the arrow keys,
      // reset with Delete/Escape. valuemin/max/now expose the position on the route to AT.
      el.setAttribute('role', 'slider');
      el.setAttribute('tabindex', '0');
      el.setAttribute('aria-label', t('gasPanel.map.dropAria', { n: i + 1 }));
      const curIndex = routeRefs.findIndex(
        (r) => r.section_id === ref.section_id && r.station_index === ref.station_index
      );
      el.setAttribute('aria-valuemin', '0');
      el.setAttribute('aria-valuemax', String(Math.max(0, routeRefs.length - 1)));
      el.setAttribute('aria-valuenow', String(curIndex < 0 ? 0 : curIndex));
      el.title = overReserve
        ? t('gasPanel.map.dropOverReserveTooltip', { n: i + 1 })
        : t('gasPanel.map.dropDraggableTooltip', { n: i + 1 });
      el.style.cssText =
        'line-height:0;cursor:grab;touch-action:none;filter:drop-shadow(0 0 1px rgba(0,0,0,0.6));';
      // Downward triangle = "drop the stage here"; red when the diver dragged it over-reserve (#246).
      const fill = overReserve ? '#d64545' : '#e08a1e';
      el.innerHTML =
        '<svg width="20" height="20" viewBox="0 0 24 24" aria-hidden="true">' +
        `<path d="M3 6h18L12 20z" fill="${fill}" stroke="#ffffff" stroke-width="1.5" ` +
        'stroke-linejoin="round"/></svg>';
      const marker = new maplibregl.Marker({ element: el, draggable: true })
        .setLngLat(lngLat)
        .addTo(mapInst);
      marker.on('dragstart', () => {
        el.style.cursor = 'grabbing';
      });
      marker.on('dragend', () => {
        el.style.cursor = 'grab';
        const snapped = snapToRoute(marker.getLngLat());
        // Released in place (or nothing to snap to): put the marker back and DON'T churn an override
        // a jiggle shouldn't create a redundant pin (which would show a reset button) or refetch.
        if (
          !snapped ||
          (snapped.section_id === ref.section_id && snapped.station_index === ref.station_index)
        ) {
          marker.setLngLat(lngLat);
          return;
        }
        // Snap the drop to the nearest route station and let the backend recompute.
        setGasStageDropOverride(i, snapped);
      });
      // Keyboard: nudge one route station deeper (Right/Up) or shallower (Left/Down); reset the drop
      // to its strategy-computed spot with Delete / Backspace / Escape.
      const nudge = (delta: number) => {
        if (routeRefs.length === 0) return;
        // Start from the pending override if it's still on the route (so rapid presses before the
        // recompute lands compose), else from the rendered drop's position.
        const pending = useGasPlanStore.getState().stageDropOverrides[i];
        const pendingIdx = pending
          ? routeRefs.findIndex(
              (r) => r.section_id === pending.section_id && r.station_index === pending.station_index
            )
          : -1;
        const from = pendingIdx >= 0 ? pendingIdx : curIndex < 0 ? 0 : curIndex;
        const j = Math.min(routeRefs.length - 1, Math.max(0, from + delta));
        if (j === from) return;
        const target = routeRefs[j];
        if (!target) return;
        pendingDropFocusRef.current = i;
        setGasStageDropOverride(i, target);
      };
      const resetDrop = () => {
        if (useGasPlanStore.getState().stageDropOverrides[i] == null) return false;
        pendingDropFocusRef.current = i;
        setGasStageDropOverride(i, null);
        return true;
      };
      // stopPropagation so the drop's own keys don't also trigger document-level map shortcuts.
      el.addEventListener('keydown', (e) => {
        if (e.key === 'ArrowRight' || e.key === 'ArrowUp') {
          e.preventDefault();
          e.stopPropagation();
          nudge(1);
        } else if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') {
          e.preventDefault();
          e.stopPropagation();
          nudge(-1);
        } else if (e.key === 'Backspace' || e.key === 'Delete' || e.key === 'Escape') {
          // Only swallow the key when there's actually an override to reset, so a bare Escape
          // still bubbles (e.g. to clear a selection) when the drop is at its default.
          if (resetDrop()) {
            e.preventDefault();
            e.stopPropagation();
          }
        }
      });
      // Double-click resets a moved drop to its strategy-computed spot (mouse affordance).
      el.addEventListener('dblclick', (e) => {
        e.preventDefault();
        e.stopPropagation();
        resetDrop();
      });
      if (pendingDropFocusRef.current === i) {
        el.focus();
        pendingDropFocusRef.current = null;
      }
      gasDropMarkersRef.current.push(marker);
    });

    const finalRef = gasReachField.final_turn_ref;
    if (finalRef) {
      const lngLat = lngLatFor(finalRef);
      if (lngLat) {
        const el = document.createElement('div');
        el.setAttribute('data-testid', 'gas-final-turn-marker');
        el.title = t('gasPanel.map.finalTurnTooltip');
        el.style.cssText =
          'width:16px;height:16px;border-radius:50%;border:3px solid #ffffff;' +
          'background:#7b2fbf;box-shadow:0 0 4px rgba(0,0,0,0.7);pointer-events:none;';
        gasFinalTurnMarkerRef.current = new maplibregl.Marker({ element: el })
          .setLngLat(lngLat)
          .addTo(mapInst);
      }
    }

    return () => {
      for (const m of gasDropMarkersRef.current) m.remove();
      gasDropMarkersRef.current = [];
      gasFinalTurnMarkerRef.current?.remove();
      gasFinalTurnMarkerRef.current = null;
    };
    // NB: intentionally NOT keyed on `gasStageDropOverrides`: rebuild the ▼ markers only when the
    // recomputed FIELD lands (mirrors the turn markers). Keying on the override would rebuild against
    // the still-stale field first, consuming `pendingDropFocusRef` before the recompute and losing
    // keyboard focus; overrides are read fresh from the store where needed. The fetch effect above
    // IS keyed on the overrides, so a pin/reset always re-fetches and lands here via `gasReachField`.
  }, [showGasPlan, gasReachField, setGasStageDropOverride, activeStations, gpsOrigin, mapLoaded, t]);

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
          showToast(t('map.geometry.computeFailed'), 'geometry-compute-failed');
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

  // The adjusted view is read-only — and so is the whole map in Plan/Export, even on a
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

  // Update callbacks when they change
  useEffect(() => {
    if (surveyLayerRef.current) {
      surveyLayerRef.current.setCallbacks(toolCallbacks);
    }
  }, [toolCallbacks]);

  // Sync config with SurveyLayer for visual and hitbox layer updates
  useEffect(() => {
    if (surveyLayerRef.current && config) {
      surveyLayerRef.current.setConfig(config);
    }
  }, [config, mapLoaded]);

  // Apply theme colors to SurveyLayer and map background when resolvedTheme changes
  // MapLibre GL JS does NOT read CSS variables — colors must be re-applied explicitly
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

  // Sync selection state with SurveyLayer for visual feedback
  useEffect(() => {
    if (!surveyLayerRef.current) return;

    if (selection?.type === 'station' && selection.stationRef) {
      surveyLayerRef.current.setSelectedStation(selection.stationRef);
      surveyLayerRef.current.setSelectedShot(null, null);
      surveyLayerRef.current.setSelectedSection(null);
    } else if (selection?.type === 'shot' && selection.sectionId !== undefined && selection.shotId !== undefined) {
      surveyLayerRef.current.setSelectedStation(null);
      surveyLayerRef.current.setSelectedShot(selection.sectionId, selection.shotId);
      surveyLayerRef.current.setSelectedSection(null);
    } else if (selection?.type === 'section' && selection.sectionId !== undefined) {
      // Section selection
      surveyLayerRef.current.setSelectedStation(null);
      surveyLayerRef.current.setSelectedShot(null, null);
      surveyLayerRef.current.setSelectedSection(selection.sectionId);
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

  // Sync station index labels visibility with SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowStationLabels(showStationLabels);
  }, [showStationLabels, mapLoaded]);

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

  // Sync anchored section IDs with SurveyLayer for fading
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setAnchoredSections(anchoredSectionIds);
  }, [anchoredSectionIds, mapLoaded]);

  // Sync immovable feature IDs with SurveyLayer for disabled visual
  useEffect(() => {
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

  // Clear reference shot selection when reference shots visibility changes to OFF
  // Uses composite identity (file_path or created_at) since Project type has no id field
  const projectIdentity = project?.file_path ?? project?.created_at;
  useEffect(() => {
    if (!showReferenceShots && project?.sections) {
      filterSelection((s) => {
        if (s.type !== 'shot') return true;
        const section = project.sections.find((sec) => sec.id === s.sectionId);
        const shot = section?.shots.find((sh) => sh.id === s.shotId);
        return !shot?.is_reference;
      });
      surveyLayerRef.current?.clearShotHover();
      setHoveredShot(null);
    }
  }, [showReferenceShots, projectIdentity, filterSelection, setHoveredShot]);

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

    const cursors: Record<string, string> = {
      select: 'default',
      pan: 'grab',
      zoom: 'zoom-in',
      merge: 'cell',
      measure: 'crosshair',
      move_station: 'move',
      move_segment: 'move',
      move_shot: 'move',
      comment: 'crosshair',
      move_comment: 'crosshair',
    };

    // If hovering over a feature, SurveyLayer sets cursor to 'pointer'
    // Otherwise, set cursor based on active tool
    // Force zoom-in cursor for zoom tool even when hovering over features
    if (!isHoveringFeature || activeTool === 'zoom') {
      // In supported tools with Ctrl held, show grab cursor for panning
      if (CTRL_PAN_TOOLS.includes(activeTool) && ctrlHeld) {
        map.current.getCanvas().style.cursor = 'grab';
      } else {
        map.current.getCanvas().style.cursor = cursors[activeTool] ?? 'default';
      }
    }
  }, [activeTool, ctrlHeld, isHoveringFeature, placementMode]);

  // Disable map drag pan for tools that need custom interaction
  // (Ctrl+drag panning is handled by custom implementation, not dragPan)
  useEffect(() => {
    if (!map.current) return;

    const noPanTools = ['select', 'zoom', 'merge', 'measure', 'move_station', 'move_segment', 'move_shot', 'comment', 'move_comment'];

    if (noPanTools.includes(activeTool)) {
      map.current.dragPan.disable();
    } else {
      map.current.dragPan.enable();
    }
  }, [activeTool]);

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
        showToast('Failed to load anchored segments. Move Segment is temporarily blocked.', 'anchored-ids-error');
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

  // Track Ctrl key for cursor updates and box/zoom select cancellation
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Control' && !e.repeat) {
        setCtrlHeld(true);

        // Cancel any active box select or zoom box when Ctrl is pressed (mid-drag Ctrl converts to pan)
        setBoxSelect({ start: null, end: null });
        setZoomBox({ start: null, end: null });
      }
    };
    const handleKeyUp = (e: KeyboardEvent) => {
      if (e.key === 'Control') {
        // WORKAROUND: Tauri/WebKit fires spurious Ctrl keyup events at the moment of
        // mousedown/mouseup while holding Ctrl. Only suppress keyups that occur within
        // 50ms of a mouse event during a Ctrl+drag — genuine Ctrl releases go through
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
      // Cancel any active box select or zoom box
      setBoxSelect({ start: null, end: null });
      setZoomBox({ start: null, end: null });
    };

    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);
    window.addEventListener('blur', handleBlur);

    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
      window.removeEventListener('blur', handleBlur);
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
  // MapLibre's dragPan with modifiers is unreliable in Tauri/WebKit, so we track
  // mouse movement directly and call panBy() ourselves.
  useEffect(() => {
    const mapElement = mapContainer.current;
    if (!mapElement) return;

    // Track drag start state for custom panning.
    let customPanStart: { x: number; y: number } | null = null;
    let customPanButtonMask: number | null = null;

    const handleMouseDownCapture = (e: MouseEvent) => {
      const isCtrlLeftPan = e.ctrlKey && e.button === 0;
      const isMiddlePan = e.button === 1;
      if ((isCtrlLeftPan || isMiddlePan) && CTRL_PAN_TOOLS.includes(activeTool) && map.current) {
        customPanActiveRef.current = true;
        ctrlPanActiveRef.current = isCtrlLeftPan;
        if (isCtrlLeftPan) {
          ctrlPanMouseEventTimeRef.current = Date.now();
        }
        customPanStart = { x: e.clientX, y: e.clientY };
        customPanButtonMask = isMiddlePan ? 4 : 1;

        // Suppress default browser behavior during custom pan
        e.stopPropagation();
        e.preventDefault();

        // Change cursor to indicate panning
        map.current.getCanvas().style.cursor = 'grabbing';
      }
    };

    // Implement custom panning since modified/middle drag interactions are handled manually.
    const handleMouseMoveCapture = (e: MouseEvent) => {
      if (customPanStart && customPanButtonMask !== null && (e.buttons & customPanButtonMask) !== 0 && map.current) {
        const dx = e.clientX - customPanStart.x;
        const dy = e.clientY - customPanStart.y;

        // Pan the map by the delta (negative because we're dragging the "ground")
        if (Math.abs(dx) > 1 || Math.abs(dy) > 1) {
          map.current.panBy([-dx, -dy], { animate: false });
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

    const handleMouseUpCapture = (e: MouseEvent) => {
      if (customPanStart && map.current) {
        customPanStart = null;
        customPanButtonMask = null;
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
        // Restore cursor: if Ctrl still held, show grab cursor for next potential pan
        // Otherwise restore the tool cursor
        if (e.ctrlKey && CTRL_PAN_TOOLS.includes(activeTool)) {
          map.current.getCanvas().style.cursor = 'grab';
        } else {
          const cursors: Record<string, string> = {
            select: 'default',
            pan: 'grab',
            zoom: 'zoom-in',
            draw: 'crosshair',
            delete: 'not-allowed',
            merge: 'cell',
            measure: 'crosshair',
            move_section: 'move',
            move_station: 'default',
            move_segment: 'move',
            move_shot: 'default',
            comment: 'crosshair',
            move_comment: 'crosshair',
          };
          map.current.getCanvas().style.cursor = cursors[activeTool] ?? 'default';
        }
      }
    };

    // Capture the DOM click event during/after a custom pan gesture.
    // The browser fires 'click' as a separate event after mouseup — stopping propagation
    // on mousedown/mouseup does NOT prevent click from being dispatched. MapLibre listens
    // for 'click' on the canvas, so we must capture it to prevent spurious map clicks.
    const handleClickCapture = (e: MouseEvent) => {
      if (customPanActiveRef.current || placementJustConfirmedRef.current) {
        e.stopPropagation();
        e.preventDefault();
      }
    };

    // Capture pointer events during custom pan — MapLibre may use pointer events
    // internally, so capturing only mouse events is insufficient to prevent click leakage
    const handlePointerDownCapture = (e: PointerEvent) => {
      const isCtrlLeftPan = e.ctrlKey && e.button === 0;
      const isMiddlePan = e.button === 1;
      if ((isCtrlLeftPan || isMiddlePan) && CTRL_PAN_TOOLS.includes(activeTool) && map.current && e.pointerType === 'mouse') {
        e.stopPropagation();
      }
    };

    const handlePointerUpCapture = (e: PointerEvent) => {
      // If we're in custom pan, stop pointer events from reaching MapLibre.
      // (customPanStart is still set here — cleared by handleMouseUpCapture which fires after)
      if (customPanStart && e.pointerType === 'mouse') {
        e.stopPropagation();
      }
    };

    // Track if mouse is currently down (for filtering spurious keyup)
    let mouseIsDown = false;

    const handleMouseDownGlobal = () => {
      mouseIsDown = true;
    };

    const handleMouseUpGlobal = () => {
      mouseIsDown = false;
      // Clear pan state in case mouse was released outside the map container
      // (handleMouseUpCapture only fires for events on the container element)
      customPanStart = null;
      customPanButtonMask = null;
      if (ctrlPanActiveRef.current) {
        ctrlPanMouseEventTimeRef.current = Date.now();
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
    mapElement.addEventListener('mousemove', handleMouseMoveCapture, { capture: true });
    mapElement.addEventListener('mouseup', handleMouseUpCapture, { capture: true });
    mapElement.addEventListener('click', handleClickCapture, { capture: true });
    mapElement.addEventListener('pointerdown', handlePointerDownCapture, { capture: true });
    mapElement.addEventListener('pointerup', handlePointerUpCapture, { capture: true });
    window.addEventListener('mousedown', handleMouseDownGlobal, { capture: true });
    window.addEventListener('mouseup', handleMouseUpGlobal);

    return () => {
      mapElement.removeEventListener('mousedown', handleMouseDownCapture, { capture: true });
      mapElement.removeEventListener('mousemove', handleMouseMoveCapture, { capture: true });
      mapElement.removeEventListener('mouseup', handleMouseUpCapture, { capture: true });
      mapElement.removeEventListener('click', handleClickCapture, { capture: true });
      mapElement.removeEventListener('pointerdown', handlePointerDownCapture, { capture: true });
      mapElement.removeEventListener('pointerup', handlePointerUpCapture, { capture: true });
      window.removeEventListener('mousedown', handleMouseDownGlobal, { capture: true });
      window.removeEventListener('mouseup', handleMouseUpGlobal);
    };
  }, [activeTool]);

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
  // Uses custom stalagmight-tiles:// protocol for offline caching
  const getSatelliteTileConfig = useCallback(() => {
    return getTileConfig();
  }, []);

  // Handle satellite toggle
  useEffect(() => {
    if (!map.current || !mapLoaded) return;

    // Store reference for cleanup
    const currentMap = map.current;
    let errorHandler: ((e: { error?: { status?: number }; sourceId?: string }) => void) | null = null;

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

        // Add satellite layer below survey layers but above background
        // 'survey-shots-hitbox' is the first/bottom survey layer (hitbox layers are added first)
        const firstSurveyLayer = currentMap.getLayer('survey-shots-hitbox') ? 'survey-shots-hitbox' : undefined;
        currentMap.addLayer({
          id: 'satellite-tiles',
          type: 'raster',
          source: 'satellite-tiles',
          paint: {
            'raster-opacity': 0.7,
          },
        }, firstSurveyLayer);

        // Add error handler for offline fallback
        const sourceId = 'satellite-tiles';
        errorHandler = (e: { error?: { status?: number }; sourceId?: string }) => {
          if (e.sourceId === sourceId) {
            console.warn('[MapCanvas] Satellite tile load failed - tiles may be unavailable offline');
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
  // sized to exclude the side panels), so no panel offsets are needed — adding
  // them double-counts the panel widths and squeezes the fit (see #64).
  const calcPadding = useCallback((container: HTMLElement) => {
    const marginX = Math.round(container.clientWidth * 0.1);
    const marginY = Math.round(container.clientHeight * 0.1);
    return { top: marginY, bottom: marginY, left: marginX, right: marginX };
  }, []);

  // Fit to survey bounds using transformed coordinates
  const fitToBounds = useCallback((options?: { snap?: boolean }) => {
    // Frame the *active* (drawn) geometry — adjusted in the closure-adjusted view.
    if (!map.current || activeStations.length === 0) return;

    const coords = activeStations.map((s) => s.coordinates);
    const bounds = getBounds(coords);

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
  }, [activeStations, gpsOrigin, calcPadding, config]);

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
      // stations, falling back to all stations when nothing matches. When
      // reference shots are hidden, a long ref/jump connector or a ref-only
      // station shouldn't inflate the bounds and shrink the section (mirrors
      // the export, which never draws reference shots). But when reference
      // shots are *visible* (the default), they're drawn — so include them, or
      // a visible connector could be left off-screen.
      const refShotsVisible = getEffectiveShowReferenceShots(showShots, showReferenceShots);
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
        if (!refShotsVisible && shot.is_reference) return;
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
      // column, so it already excludes the side panels — adding the panel
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
      // padding and container — not the live (possibly mid-animation) camera —
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
    [activeStations, project?.sections, config, gpsOrigin, showShots, showReferenceShots, calcPadding]
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
  // button (#57) — clicking repeatedly closes in on the problematic shot.
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
        clearFitToBoundsRequest();
      } else {
        // Small delay to ensure data is rendered
        setTimeout(() => {
          fitToBounds();
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
      // Disable drag pan during placement
      map.current.dragPan.disable();
    } else {
      // Re-enable drag pan when placement mode ends (unless current tool should disable it)
      const noPanTools = ['select', 'zoom', 'merge', 'measure', 'move_station', 'move_segment', 'move_shot', 'comment', 'move_comment'];
      if (!noPanTools.includes(activeTool)) {
        map.current.dragPan.enable();
      }
      // Reset cursor based on current tool
      const cursors: Record<string, string> = {
        select: 'default',
        pan: 'grab',
        zoom: 'zoom-in',
        merge: 'crosshair',
        measure: 'crosshair',
        move_section: 'move',
        move_station: 'move',
        move_segment: 'move',
        move_shot: 'move',
        comment: 'crosshair',
        move_comment: 'crosshair',
      };
      map.current.getCanvas().style.cursor = cursors[activeTool] ?? 'default';
    }
  }, [placementMode, activeTool]);

  // Determine touch-action: block browser gestures for interactive tools, allow pan/zoom for pan tool
  const touchAction = useMemo(() => {
    const noPanTools = ['select', 'zoom', 'merge', 'measure',
      'move_station', 'move_segment', 'move_shot', 'comment', 'move_comment'];
    return (placementMode || noPanTools.includes(activeTool)) ? 'none' : 'manipulation';
  }, [activeTool, placementMode]);

  // Calculate box select overlay style
  const boxSelectStyle = boxSelect.start && boxSelect.end
    ? {
        left: Math.min(boxSelect.start.x, boxSelect.end.x),
        top: Math.min(boxSelect.start.y, boxSelect.end.y),
        width: Math.abs(boxSelect.end.x - boxSelect.start.x),
        height: Math.abs(boxSelect.end.y - boxSelect.start.y),
      }
    : null;

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

      if (moveDrag) {
        handleMoveDragMove(e.clientX, e.clientY);
      } else if (zoomBox.start) {
        handleZoomBoxMove(e);
      } else if (boxSelect.start) {
        handleBoxSelectMove(e);
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
    [moveDrag, zoomBox.start, boxSelect.start, handleMoveDragMove, handleZoomBoxMove, handleBoxSelectMove, checkSnapToStation, placementMode, checkPlacementSnap, updatePlacementCursor, updatePlacementSnap]
  );

  const handlePointerUp = useCallback(() => {
    if (moveDrag) {
      // Block while snap-to-merge dialog is open
      if (snapMergeDialogPendingRef.current) {
        return;
      }
      handleMoveDragEnd();
    } else if (zoomBox.start) {
      handleZoomBoxEnd();
    } else if (boxSelect.start) {
      handleBoxSelectEnd();
    }
  }, [moveDrag, zoomBox.start, boxSelect.start, handleMoveDragEnd, handleZoomBoxEnd, handleBoxSelectEnd]);

  // Pointer leave: keep all move drags alive so user can return the cursor
  const handlePointerLeave = useCallback(() => {
    if (moveDrag) return;
    handlePointerUp();
  }, [moveDrag, handlePointerUp]);

  // Handle placement mode click
  const handlePlacementClick = useCallback(
    (e: React.PointerEvent) => {
      if (!placementMode || !map.current || !mapContainer.current) return;

      // Left click confirms placement
      if (e.button === 0) {
        e.preventDefault();
        e.stopPropagation();

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
    [placementMode, handlePlacementConfirm, gpsOrigin, updatePlacementCursor, updatePlacementSnap, checkPlacementSnap]
  );

  // Handle placement mode right-click (cancel)
  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      if (placementMode) {
        e.preventDefault();
        e.stopPropagation();
        handlePlacementCancel();
      }
    },
    [placementMode, handlePlacementCancel]
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
      // Let MapLibre handle touch pan/zoom when pan tool is active
      if (e.pointerType === 'touch' && activeTool === 'pan') {
        return;
      }
      if (placementMode) {
        handlePlacementClick(e);
      } else if (activeTool === 'zoom') {
        handleZoomBoxStart(e);
      } else {
        handleBoxSelectStart(e);
      }
    },
    [placementMode, activeTool, handlePlacementClick, handleZoomBoxStart, handleBoxSelectStart]
  );

  // Pointer handlers always attached - they check e.ctrlKey internally to decide behavior
  return (
    <div
      className="relative w-full h-full"
      style={{ touchAction }}
      data-testid="map-canvas"
      data-map-loaded={mapLoaded ? 'true' : 'false'}
      data-active-tool={activeTool}
      data-placement-mode={placementMode ? 'true' : 'false'}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerLeave={handlePointerLeave}
      onContextMenu={handleContextMenu}
    >
      <div ref={mapContainer} className="w-full h-full" data-testid="maplibre-container" />

      {/* Box select overlay */}
      {boxSelectStyle && (
        <div
          className="absolute border-2 border-accent border-dashed bg-accent-10 pointer-events-none"
          style={boxSelectStyle}
          data-testid="box-select-overlay"
        />
      )}

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

      {/* Depth legend when depth gradient is enabled (suppressed while the
          distance overlay is active — distance wins visually, one legend). */}
      {effectiveDepthGradient && !showDistanceGradient && (
        <DepthLegend
          minDepth={depthRange.min}
          maxDepth={depthRange.max}
          visible={effectiveDepthGradient}
        />
      )}

      {/* Distance legend (#34): color key + "which overlay" cue. Hidden while
          issue-severity owns the paint (e.g. the Warnings tab) so the legend
          matches the visible overlay (#57 / Codex). */}
      {showDistanceGradient && !effectiveIssueSeverity && distanceField && (
        <DistanceLegend max={distanceField.max_reachable} visible={showDistanceGradient} />
      )}
      {/* While the field is being (re)computed — first toggle / re-root — show a
          lightweight cue so a slow closure solve doesn't look frozen. */}
      {showDistanceGradient && !effectiveIssueSeverity && !distanceField && (
        <div
          data-testid="distance-legend-loading"
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
          {t('map.distanceLegend.computing')}
        </div>
      )}

      {/* Gas reachable-frontier legend (#38) — gold→purple, 0 → turn budget. */}
      {showGasPlan && gasReachField && (
        <GasLegend field={gasReachField} units={gasPlanConfig.units} visible={showGasPlan} />
      )}
      {showGasPlan && !gasReachField && (
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

      {/* Developer stats overlay */}
      {showDevStats && mapLoaded && map.current && (
        <MapDevStats map={map.current} />
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

      {/* Fit-to-view button (+ exit-focus, #50). The SpeleoDB read-only badge (T2b)
          sits to its RIGHT in the same top-left flex row when a remote project is
          open without the lock - the flex gap tracks the button's variable width
          (it widens with the exit-focus label). */}
      <div className="absolute top-2 left-2 flex items-center gap-2">
        <button
          onClick={() => (focusedSectionId !== null ? exitSectionFocus() : fitToBounds())}
          className="flex items-center gap-1.5 bg-panel border border-border rounded px-2 py-1 text-sm hover:bg-background"
          title={focusedSectionId !== null ? t('sections.focus.exit') : t('map.fitToViewTitle')}
          aria-label={focusedSectionId !== null ? t('sections.focus.exit') : t('map.fitToViewTitle')}
          data-testid="fit-to-view-button"
        >
          <FitToViewIcon size={16} />
          {/* In focus mode the button also exits focus - label it so that's clear. */}
          {focusedSectionId !== null && <span>{t('sections.focus.exitLabel')}</span>}
        </button>
        {speleoReadOnly && !placementMode && appMode === 'survey' && (
          <div
            className="bg-panel border border-accent rounded px-3 py-1.5 text-xs flex items-center gap-1.5"
            data-testid="speleodb-readonly-banner"
          >
            <LockIcon size={13} />
            <span className="text-accent font-medium">{t('speleodb.map.readOnlyBadge')}</span>
          </div>
        )}
      </div>

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
              ? '—'
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

      {/* Move drag indicator */}
      {moveDrag && !placementMode && (
        <div className="absolute top-2 left-1/2 -translate-x-1/2 bg-panel border border-accent rounded px-3 py-2 text-sm">
          <span className="text-accent">
            {moveDrag.type === 'station'
              ? t('map.move.station')
              : moveDrag.type === 'segment'
                ? t('map.move.segment')
                : t('map.move.shot')}
            :
          </span>{' '}
          {t('map.move.instructions')}
        </div>
      )}

      {/* Closure-adjusted read-only banner (+ "computing" while solving) */}
      {geometryMode === 'closureAdjusted' && !placementMode && (
        <div
          className="absolute top-2 left-2 bg-panel border border-accent rounded px-3 py-1.5 text-xs flex items-center gap-2"
          data-testid="geometry-readonly-banner"
        >
          <span className="text-accent font-medium">{t('map.geometry.adjustedBadge')}</span>
          {adjustedLoading && (
            <span className="text-text-secondary" data-testid="geometry-computing">
              {t('map.geometry.computing')}
            </span>
          )}
        </div>
      )}

      {/* Comment tool overlay */}
      <CommentOverlay />

      {/* Move comment cursor tag — shows picked-up comment following cursor */}
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
// must NOT re-render just because a parent (SingleSystemView) re-renders — e.g.
// on every tick of a panel resize. Without this the whole (large) map component
// re-rendered ~60×/s during a drag, which is what made resizing feel heavy.
export default memo(MapCanvas);
