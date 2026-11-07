import { useEffect, useLayoutEffect, useRef, useCallback, useState, useMemo, memo } from 'react';
import { useTranslation } from 'react-i18next';
import * as maplibregl from 'maplibre-gl';
import type { ProjectWarning } from '../../types/commands';
import { useProjectStore } from '../../store/projectStore';
import {
  useUIStore,
  isMapReadOnly,
  showsClosureAdjustedNotice,
} from '../../store/uiStore';
import { pickUIKeys, type UIStoreState } from '../../store/uiSelectKeys';
import { selectComputeGate, useGasPlanStore } from '../../store/gasPlanStore';
import { selectActiveStations, selectActiveSurveyData } from '../../store/geometrySelectors';
import { isMutatingTool, shouldResetNavigationTool } from '../../utils/toolUtils';
import { mapGestureMode } from '../../utils/mapGestures';
import { hasEligibleClosures } from '../../utils/closures';
import { useShallow } from 'zustand/react/shallow';
import { isShotKindVisible } from '../../store/displaySelectors';
import { buildMeasuredPathIds, buildMeasuredStationIds, MEASURE_TINT } from '../../utils/measurePath';
import { computeImmovableStationKeys } from '../../utils/immovableStations';
import { installMapE2eHooks } from '../../dev/mapE2eHooks';
import { getBoundsFromStationCoords, metersToMapCoords, mapCoordsToMeters } from '../../utils/coordinates';
import { getTileConfig } from '../../utils/tileConfig';
import { useMapTools } from '../../hooks/useMapTools';
import { useGasPlanPointMarkers } from './useGasPlanPointMarkers';
import { useGasPlanDropMarkers } from './useGasPlanDropMarkers';
import { useMoveCarryGuards } from './useMoveCarryGuards';
import { MapHoverSync } from './MapHoverSync';
import { useStationSnap } from './useStationSnap';
import { useMoveDrag } from './useMoveDrag';
import { usePlacement } from './usePlacement';
import { useAnalyticalOverlays } from './useAnalyticalOverlays';
import { useSurveyLayerSync } from './useSurveyLayerSync';
import { useZoomBox } from './useZoomBox';
import { CUSTOM_PAN_CLAIM_PX, useModifierPan } from './useModifierPan';
import { useMapCamera } from './useMapCamera';
import SurveyLayer, { type SurveyLayerCallbacks } from './SurveyLayer';
import { mapCursorFor } from './mapCursor';
import { createSurveyMap, SATELLITE_SOURCE_ID } from './createSurveyMap';
import { pixelsPerMeterAt } from './placementScale';
import { useContextLossToast } from './useContextLossToast';
import { useStyleEpoch } from './useStyleEpoch';
import { SATELLITE_BOTTOM_CANDIDATES, canWriteStyle } from './mapUtils';
import { renderFatalMessage } from './surveyLayer/mapErrors';
import PlacementOverlay from './PlacementOverlay';
import GridOverlay from './GridOverlay';
import { DepthLegend } from './DepthLegend';
import { DistanceLegend } from './DistanceLegend';
import { GasLegend } from './GasLegend';
import { MapDevStatsPanels } from './MapDevStatsPanels';
import CommentOverlay from './CommentOverlay';
import MoveCommentCursorTag from './MoveCommentCursorTag';
import { installPanZoomDebug } from './panZoomDebug';
import { MapUnavailable } from './MapUnavailable';
import { getMapThemeColors } from '../../utils/themeColors';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { MapTopLeftOverlay } from './MapTopLeftOverlay';
import { useSpeleoStore, selectSpeleoReadOnly } from '../../store/speleoStore';
import { FEATURE_ID_STRIDE } from '../../utils/ids';
import { debugLog } from '../../utils/debugLog';
import { TOAST_KEYS } from '../../utils/toastKeys';

// Stable empty warnings array fed to the survey-layer rebuild effect when
// Issue-severity coloring is OFF. Module-level so its identity never changes
// across renders - a per-render [] would defeat the rebuild-gating memo below.
const EMPTY_WARNINGS: ProjectWarning[] = [];

// The same idiom for the move-tool dim inputs: outside the move tools every edit
// re-ran their memos / the anchored effect, and fresh empty sets re-fired the
// SurveyLayer dim sync each time (#576 D8). Shared, so never mutated.
const NO_STATION_KEYS = new Set<string>();
const NO_SECTION_IDS = new Set<number>();
const NO_IMMOVABLE_FEATURES = { stationIds: new Set<number>(), shotIds: new Set<number>() };

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

/** A map center in the map's own coordinates. */
type MapCenter = [number, number];

interface MapCanvasProps {
  /** Rebuild the map on a new WebGL context (the lost-context toast's action),
   *  handing over the camera center the rebuilt map starts from. */
  onReloadMap: (center: MapCenter | null) => void;
  /** Where a rebuilt map starts (#576 RV4-FE-MF-3); null: the survey's center. */
  initialCenter: MapCenter | null;
}

function MapCanvas({ onReloadMap, initialCenter }: MapCanvasProps) {
  const precision = useDecimalPrecision();
  const mapContainer = useRef<HTMLDivElement>(null);
  const map = useRef<maplibregl.Map | null>(null);
  // Read once, by the map-creation effect (empty deps, like every input it reads).
  const initialCenterRef = useRef(initialCenter);
  const surveyLayerRef = useRef<SurveyLayer | null>(null);
  const mapInteractionSequenceRef = useRef(0);
  const scaleControlRef = useRef<maplibregl.ScaleControl | null>(null);
  const scaleControlAddedRef = useRef<boolean>(false);
  const [mapLoaded, setMapLoaded] = useState(false);
  // Moves once the style is back after a WebGL context restore: style-writing
  // effects that skipped while it was gone run again (#576 NF-MAP-5).
  const styleEpoch = useStyleEpoch(map, mapLoaded);
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
  // RAF batching for snap-to-station checks (prevents event queue buildup on mousemove)
  const snapRafId = useRef<number | null>(null);
  const pendingSnapPosition = useRef<{ x: number; y: number } | null>(null);
  // Anchored section IDs (cached when Move Segment tool is active)
  const [anchoredSectionIds, setAnchoredSectionIds] = useState<Set<number>>(NO_SECTION_IDS);
  // Whether anchored section IDs have been loaded for the current move_segment activation
  // Blocks segment drag until the set is populated to prevent moving anchored segments
  const [anchoredIdsLoaded, setAnchoredIdsLoaded] = useState(false);
  // The carry registered with src/store/moveCarry.ts (GHOST-2), from the pickup click
  // until the drop starts its commit or the carry is cancelled.
  const moveCarryTokenRef = useRef<number | null>(null);
  useMoveCarryGuards(moveCarryTokenRef);
  // Where the live press landed (client coords), or null once that press has
  // travelled far enough to be a drag rather than a click. This is what
  // click-gates the move ghost's drop (#409): a plain drag pans the map, so only
  // a press that stays put may commit. Set on every pointerdown that reaches the
  // canvas, cleared by the release that ends it - and by the first pointermove
  // past the threshold, which latches the decision exactly as the custom pan's
  // own claim does (coming back to the press point cannot revive the drop).
  const pressClientRef = useRef<{ x: number; y: number } | null>(null);
  const { t } = useTranslation();

  const {
    project,
    stations,
    surveyData,
    stationsAdjusted,
    surveyDataAdjusted,
    adjustedLoading,
    loadAdjustedGeometry,
    config,
    warnings,
    moveStation,
    moveSegment,
    getConnectedSectionIds,
    getAnchoredSectionIds,
    executeBatch,
  } = useProjectStore(
    useShallow((s) => ({
      project: s.project,
      stations: s.stations,
      surveyData: s.surveyData,
      stationsAdjusted: s.stationsAdjusted,
      surveyDataAdjusted: s.surveyDataAdjusted,
      adjustedLoading: s.adjustedLoading,
      loadAdjustedGeometry: s.loadAdjustedGeometry,
      config: s.config,
      warnings: s.warnings,
      moveStation: s.moveStation,
      moveSegment: s.moveSegment,
      getConnectedSectionIds: s.getConnectedSectionIds,
      getAnchoredSectionIds: s.getAnchoredSectionIds,
      executeBatch: s.executeBatch,
    })),
  );
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
  // For the map's own 'zoom' handler, registered once at map creation.
  const gpsOriginRef = useRef(gpsOrigin);
  useLayoutEffect(() => {
    gpsOriginRef.current = gpsOrigin;
  }, [gpsOrigin]);

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
  // `placementMode` is replaced wholesale on every placement pointer move, so
  // effects and callbacks that only ask "is placement on?" take this boolean;
  // depending on the object re-registered their listeners ~60x/s (#576 D9).
  // Those that need its payload read it from the store when they run.
  const placementActive = placementMode !== null;

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
  // Gate the survey-layer rebuild on issue-severity coloring. `warnings` changes
  // identity on every refreshWarnings (each edit/resolve/ignore), but warnings
  // only affect colors when severity coloring is effectively ON - feed the rebuild
  // a stable shared [] when OFF so warning-only refreshes don't rebuild the GeoJSON.
  const severityWarnings = useMemo(
    () => (effectiveIssueSeverity ? warnings : EMPTY_WARNINGS),
    [effectiveIssueSeverity, warnings]
  );

  const alignedClosureIds = useMemo(() => {
    if (!project?.loop_closures) return new Set<number>();
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
    if (activeTool !== 'move_station' && activeTool !== 'move_shot') return NO_STATION_KEYS;
    if (!surveyData) return NO_STATION_KEYS;
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
      return NO_IMMOVABLE_FEATURES;
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

  // The merge / measure cursor snap (useStationSnap.ts): the frame-rate snap the
  // RAF-batched pointer move below runs, the click-time re-query for useMapTools,
  // and the stale-snap cleanups.
  const { checkSnapToStation, querySnapAtPoint } = useStationSnap({
    map,
    surveyLayerRef,
    config,
    project,
    activeTool,
    showStations,
    showShots,
    showJumps,
    showReferenceShots,
    snappedStation,
    setSnappedStation,
    setMeasureSnapShot,
  });

  // The move tools' pickup / drag / drop / snap-merge (useMoveDrag.ts). The carry
  // token above and the drop's click gate (`pressClientRef`) stay here.
  const {
    moveDrag,
    moveDragRef,
    segmentDragTokenRef,
    segmentPickupTokenRef,
    snapMergeDialogPendingRef,
    handleMoveDragStart,
    handleMoveDragMove,
    handleMoveDragEnd,
  } = useMoveDrag({
    map,
    mapContainer,
    surveyLayerRef,
    moveCarryTokenRef,
    config,
    stations,
    activeStations,
    gpsOrigin,
    immovableStationKeys,
    anchoredSectionIds,
    anchoredIdsLoaded,
    clearSelectionAndVisuals,
    getConnectedSectionIds,
    moveStation,
    moveSegment,
    executeBatch,
    setSnappedStation,
    t,
  });

  const noteMapInteraction = useCallback(() => {
    mapInteractionSequenceRef.current += 1;
  }, []);

  // Placement mode: snap, confirm, cancel, Escape and the placement click /
  // right-click (usePlacement.ts). The pixelsPerMeter seed effect stays below.
  const {
    placementCursor,
    setPlacementCursor,
    pixelsPerMeter,
    setPixelsPerMeter,
    checkPlacementSnap,
    handlePlacementCancel,
    handlePlacementClick,
    handleContextMenu,
  } = usePlacement({
    map,
    mapContainer,
    surveyLayerRef,
    config,
    gpsOrigin,
    placementActive,
    placementJustConfirmedRef,
    confirmPlacement,
    cancelPlacementMode,
    updatePlacementCursor,
    updatePlacementSnap,
    noteMapInteraction,
  });

  // Get map tool callbacks
  const baseToolCallbacks = useMapTools({
    onMoveDragStart: handleMoveDragStart,
    immovableStationKeys,
    gpsOrigin,
    querySnapAtPoint,
  });

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
        // With a move ghost in flight a right-click is inert: the feature menu
        // offers Delete / Merge / Add Shot, and any of them would change the
        // survey under the ref the ghost was picked up against. The native menu
        // stays suppressed; Escape cancels and a left still click drops. The
        // touch long-press is gated on the same ghost (SurveyLayer's
        // `modalInteractionActive`). A Move Segment pickup counts from the
        // click, not from when its connected-section fetch returns and the
        // ghost goes up, or the menu would open under that ghost.
        if (moveDragRef.current || segmentPickupTokenRef.current === segmentDragTokenRef.current) {
          event.originalEvent.preventDefault();
          return;
        }
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
    // The three refs come from useMoveDrag's useRef: stable, listed for the linter only.
    [baseToolCallbacks, noteMapInteraction, showToast, t, moveDragRef, segmentDragTokenRef, segmentPickupTokenRef],
  );

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

    const created = createSurveyMap({
      container: mapContainer.current,
      style: {
        version: 8,
        // No `glyphs`: there is no symbol layer - survey labels are the custom
        // WebGL layer with its own bundled-font atlas (glyphAtlas.ts).
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
      // A map rebuilt by "Reload map" keeps the camera it had (#576 RV4-FE-MF-3):
      // its zoom comes back through the store below, its center through the host.
      center: initialCenterRef.current ?? mapCenter,
      // Read at creation time rather than subscribed: this effect has an empty
      // dep array, so a subscription could never deliver a newer value here and
      // would only re-render MapCanvas on every 'zoomend' (#430).
      zoom: useUIStore.getState().mapZoom,
      // Disable built-in keyboard zoom (handled by useKeyboardShortcuts with config.map.zoom_step)
      keyboard: false,
      // Disable box zoom (Ctrl+drag draws selection box by default, we want Ctrl+drag to pan)
      boxZoom: false,
      performanceTuned: true,
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
      setPixelsPerMeter(pixelsPerMeterAt(map.current, gpsOriginRef.current));
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
      // Still release what survives map.remove() - window hooks, timers, frames
      // and listeners that would fire later against the dead map and pin the
      // layer until they did (#mem, #576 D16). Cheap: JS-only, no map/worker
      // work, so it doesn't reintroduce the freeze.
      surveyLayerRef.current?.release();
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

  // A WebGL context that is lost and does not come back within a few seconds
  // leaves the survey blank for good: say so, and offer the rebuild (#576 D38-LOST).
  const reloadMapKeepingCamera = useCallback(() => {
    const center = map.current?.getCenter();
    onReloadMap(center ? [center.lng, center.lat] : null);
  }, [onReloadMap]);
  useContextLossToast(map, mapLoaded, reloadMapKeepingCamera);

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

  // The distance-gradient (#34) and gas-reach (#38) overlays: field fetches, the
  // apply effects, the Plan idle tint, the picked route and both origin markers
  // (useAnalyticalOverlays.ts).
  const { distanceFieldFailed } = useAnalyticalOverlays({
    map,
    surveyLayerRef,
    mapLoaded,
    project,
    config,
    surveyData,
    activeStations,
    gpsOrigin,
    effectiveIssueSeverity,
    showDistanceGradient,
    distanceGradientOrigin,
    distanceField,
    setDistanceField,
    showGasPlan,
    gasOriginChosen,
    gasOrigin,
    gasComputeGate,
    gasWaypoints,
    gasSegments,
    gasDiver,
    gasCylinders,
    gasPlanConfig,
    gasReachField,
    setGasReachField,
  });

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

  // Store and derived state pushed into SurveyLayer (useSurveyLayerSync.ts): the
  // geometry update layout effect, callbacks, config, theme, selection, snap /
  // merge highlight, label and kind visibility, the move-comment hide and the
  // anchored / immovable dims.
  useSurveyLayerSync({
    map,
    surveyLayerRef,
    mapLoaded,
    styleEpoch,
    project,
    activeStations,
    gpsOrigin,
    effectiveDepthGradient,
    depthRange,
    precision,
    geometryMode,
    surveyDataAdjusted,
    effectiveIssueSeverity,
    severityWarnings,
    toolCallbacks,
    config,
    resolvedTheme,
    selection,
    snappedStation,
    mergeStationA,
    mergeStationB,
    showShotData,
    showShots,
    showSectionNames,
    showStationLabels,
    showStationComments,
    showStations,
    showShotComments,
    showReferenceShots,
    showJumps,
    moveCommentSource,
    anchoredSectionIds,
    immovableFeatureIds,
  });

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
      if (placementActive) {
        handlePlacementCancel();
      }
      // Note: Do NOT clear snappedStation or measure state
      // These operations work with hidden stations - highlight layer shows them
    }
  }, [showStations, clearSelectionByType, setHoveredStation, clearMergeStations, placementActive, handlePlacementCancel]);

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

  // Hand the tool, the Ctrl-held pan modifier and placement mode to the map's
  // cursor owner (mapCursor.ts, #576 D3), which weighs them against SurveyLayer's
  // hover verdict and a running modifier pan. It is the only writer of the canvas
  // cursor, so the order these inputs arrive in no longer decides what shows.
  useEffect(() => {
    if (!map.current) return;
    mapCursorFor(map.current).set({ tool: activeTool, ctrlHeld, placement: placementActive });
  }, [activeTool, ctrlHeld, placementActive]);

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
        setAnchoredSectionIds(NO_SECTION_IDS);
        showToast(t('map.toasts.anchoredSegmentsError'), TOAST_KEYS.ANCHORED_IDS_ERROR, 'error');
      });
    } else {
      setAnchoredSectionIds(NO_SECTION_IDS);
      setAnchoredIdsLoaded(false);
    }
    return () => { cancelled = true; };
  }, [activeTool, clearSelectionAndVisuals, getAnchoredSectionIds, project, showToast]);

  // Wire up setActiveTool - depend on mapLoaded to ensure it runs after layer creation
  useEffect(() => {
    if (surveyLayerRef.current) {
      // Under Pan/Zoom setActiveTool also drops the shot/station hover and, through
      // onShotHover/onStationHover, nulls the store's too: read them first.
      const { hoveredShot, hoveredStation } = useUIStore.getState();
      surveyLayerRef.current.setActiveTool(activeTool);
      // Under Pan/Zoom setActiveTool drops the layer's section hover. A switch TO
      // Pan/Zoom clears the store's in the same update, so this re-applies null;
      // but on map load this runs after MapHoverSync (a child, so its effects run
      // first) pushed in the hover of a panel row still under the pointer, which
      // must stay painted (#576 W26-NF2).
      surveyLayerRef.current.setHoveredSection(useUIStore.getState().hoveredSectionId);
      // Put back the data-table/detail row hover that clear took from store and
      // map on load. A switch TO Pan/Zoom had already cleared the store, so this
      // restores nothing and a map-pointer hover stays cleared (#576 W33-NF8).
      const ui = useUIStore.getState();
      if (ui.hoveredShot !== hoveredShot) ui.setHoveredShot(hoveredShot);
      if (ui.hoveredStation !== hoveredStation) ui.setHoveredStation(hoveredStation);
      surveyLayerRef.current.setHoveredShot(hoveredShot?.sectionId ?? null, hoveredShot?.shotId ?? null);
      surveyLayerRef.current.setHoveredStation(hoveredStation);
    }
  }, [activeTool, mapLoaded]);

  // The zoom tool's box, its Escape key and its canvas capture (useZoomBox.ts).
  const { zoomBox, setZoomBox, handleZoomBoxStart, handleZoomBoxMove, handleZoomBoxEnd } = useZoomBox({
    map,
    mapContainer,
    activeTool,
    placementActive,
    zoomStep,
    zoomMin,
    zoomMax,
  });

  // The Ctrl+left / middle custom pan and the Ctrl-held tracking that feeds the
  // cursor effect above (useModifierPan.ts).
  useModifierPan({
    map,
    mapContainer,
    config,
    activeTool,
    mapLibreDragPan,
    customPanActiveRef,
    ctrlPanActiveRef,
    ctrlPanMouseEventTimeRef,
    placementJustConfirmedRef,
    setCtrlHeld,
    setZoomBox,
  });

  // Get satellite tile URL based on provider config
  // Uses the custom stalagmight-tiles:// protocol; the backend caches under the
  // provider's own Cache-Control
  const getSatelliteTileConfig = useCallback(() => {
    return getTileConfig();
  }, []);

  // Handle satellite toggle
  useEffect(() => {
    // No style while the WebGL context is lost (reading it throws), and a rebuilt
    // one refuses writes until it has loaded; styleEpoch re-runs this once the
    // restored style has loaded (#576 NF-MAP-5, RV18-MAP-1).
    if (!map.current || !mapLoaded || !canWriteStyle(map.current)) return;

    // Store reference for cleanup
    const currentMap = map.current;
    let errorHandler: ((e: maplibregl.ErrorEvent & { sourceId?: string }) => void) | null = null;

    if (showSatellite) {
      // Add satellite layer if not already present
      if (!currentMap.getSource(SATELLITE_SOURCE_ID)) {
        const tileConfig = getSatelliteTileConfig();

        currentMap.addSource(SATELLITE_SOURCE_ID, {
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
  }, [showSatellite, getSatelliteTileConfig, mapLoaded, styleEpoch]);

  // Satellite tiles that failed offline reload when the connection returns:
  // createSurveyMap owns that for all three maps (#576 D18).

  // The camera: fit / center / pan / zoom-to helpers, the uiStore request effects
  // that drive them, and the zoom config (useMapCamera.ts).
  const { fitToBounds } = useMapCamera({
    map,
    surveyLayerRef,
    mapLoaded,
    config,
    project,
    stations,
    activeStations,
    activeSurveyData,
    gpsOrigin,
    showShots,
    showJumps,
    showReferenceShots,
    zoomStep,
    zoomMax,
    requestFitToBounds,
    clearFitToBoundsRequest,
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
  });

  // Render the transient geometry preview (issue #57): the warning-resolution
  // flow pushes a proposed ghost shot here as the user tries options, drawn via
  // the survey layer's ghost overlay (setGhostSection).
  useEffect(() => {
    if (!mapLoaded) return;
    surveyLayerRef.current?.setGhostSection(geometryPreview ?? null);
  }, [geometryPreview, mapLoaded]);

  // Seed the to-scale placement factor the moment placement mode turns on. The
  // map's 'zoom' handler keeps it live from there, but it does not fire until
  // the user actually zooms, so without this the overlay would draw at whatever
  // scale was last recorded (or the initial 1 px/m) until the first zoom.
  useEffect(() => {
    if (!placementActive || !mapLoaded || !map.current) return;
    setPixelsPerMeter(pixelsPerMeterAt(map.current, gpsOrigin));
  }, [placementActive, mapLoaded, gpsOrigin]);

  // `touchAction` - block browser gestures only where the tool or mode owns the
  // drag - comes from the single `mapGestureMode` call above.

  // Combined pointer handlers (PointerEvent is a superset of MouseEvent)
  const handlePointerMove = useCallback(
    (e: React.PointerEvent) => {
      // Handle placement mode
      if (placementActive && mapContainer.current && map.current) {
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
    [moveDrag, zoomBox.start, handleMoveDragMove, handleZoomBoxMove, checkSnapToStation, placementActive, checkPlacementSnap, updatePlacementCursor, updatePlacementSnap]
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
      //
      // Only the primary button drops, like placement's confirm and the zoom
      // box: a still right- or middle-click leaves the ghost in flight, and
      // Escape stays the way to cancel it.
      if (e.button !== 0) return;
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
      if (placementActive) {
        handlePlacementClick(e);
      } else if (activeTool === 'zoom') {
        handleZoomBoxStart(e);
      }
    },
    [placementActive, activeTool, handlePlacementClick, handleZoomBoxStart]
  );

  // What the move-tool hint banner talks about (#409), or null when there is nothing to
  // hint at. An in-flight drag wins over the active tool: `moveDrag` outlives a switch
  // AMONG the three move tools (a switch to any other tool cancels the carry, GHOST-2
  // R4), and it is what the drop instruction is about.
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
      {mapLoaded && map.current && <GridOverlay map={map.current} gpsOrigin={gpsOrigin} styleEpoch={styleEpoch} />}

      {/* Depth legend when depth gradient is enabled (suppressed while the
          distance overlay is active - distance wins visually, one legend). */}
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
      {/* While the field is being (re)computed - first toggle / re-root - show a
          lightweight cue so a slow closure solve doesn't look frozen. Not once the
          fetch has failed: nothing is being computed then (#576 C7-CUE). */}
      {showDistanceGradient && !effectiveIssueSeverity && !distanceField && !distanceFieldFailed && (
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
          azimuthReference={placementMode.azimuthReference}
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
                  : measureAlongLine?.failed
                    ? t('map.measure.pathFailed')
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

const MemoMapCanvas = memo(MapCanvas);

/**
 * Owns the map's generation: "Reload map" on the lost-WebGL-context toast bumps
 * it, which remounts MapCanvas - map.remove() on the dead context, then a fresh
 * map (new canvas, new context) built from the stores (#576 D38-LOST), at the
 * camera the old one had (#576 RV4-FE-MF-3). In-flight transient state (a
 * measurement, a move carry) does not survive the rebuild.
 */
function MapCanvasHost() {
  const [rebuild, setRebuild] = useState<{ generation: number; center: MapCenter | null }>({
    generation: 0,
    center: null,
  });
  const reloadMap = useCallback(
    (center: MapCenter | null) => setRebuild((r) => ({ generation: r.generation + 1, center })),
    [],
  );
  return <MemoMapCanvas key={rebuild.generation} onReloadMap={reloadMap} initialCenter={rebuild.center} />;
}

// Memoized: MapCanvas takes no props from its parent and drives itself from the
// stores, so it must NOT re-render just because a parent (SingleSystemView)
// re-renders - e.g. on every tick of a panel resize. Without this the whole
// (large) map component re-rendered ~60×/s during a drag, which is what made
// resizing feel heavy.
export default memo(MapCanvasHost);
