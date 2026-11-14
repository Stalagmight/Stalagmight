import { useCallback, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { MapMouseEvent } from 'maplibre-gl';
import { selectDistanceOverlayActive } from '../store/displaySelectors';
import { useUIStore, type Selection, type MeasureEndpoint } from '../store/uiStore';
import { pickUIKeys, type UIStoreState } from '../store/uiSelectKeys';
import { useProjectStore } from '../store/projectStore';
import { useGasPlanStore } from '../store/gasPlanStore';
import { useShallow } from 'zustand/react/shallow';
import { isNonInteractiveTool } from '../utils/toolUtils';
import { useActiveStations } from './useActiveStations';
import { getErrorMessage, invokeIpc } from '../utils/ipc';
import type { SectionId, ShotId, StationRef, Coordinates, RulerEndpoint, AlongLineResult } from '../types/project';
import type { GpsOrigin } from '../types/config';
import { distance, mapCoordsToMeters } from '../utils/coordinates';
import { reachableStationKeySet, nearestOriginShotEndpoint } from '../utils/gasReach';
import { setSource } from '../utils/telemetry';
import type { SurveyLayerCallbacks } from '../components/Map/SurveyLayer';
import { projectParamOnSegment } from '../components/Map/surveyLayer/hitTest';
import { TOAST_KEYS } from '../utils/toastKeys';

/**
 * #395: what a plain Plan-mode select-click does to the gas plan pins.
 *
 * Three states, checked in order: the one-shot "Pick on map" origin picker (armed from the
 * Origin section's button) always wins and disarms itself via `setOrigin`; before an origin is
 * chosen at all, the FIRST click sets the origin (answering "Where does this dive start?") so
 * the entry card can resolve before any endpoint exists to plan toward; only once an origin is
 * chosen does a plain click resume its ordinary job of setting the ENDPOINT.
 */
function applyGasPlanClick(ref: StationRef): void {
  const gas = useGasPlanStore.getState();
  if (gas.originPickArmed || !gas.originChosen) {
    gas.setOrigin(ref);
    return;
  }
  gas.setEndpoint(ref);
}

/** A measure endpoint's 3D coords (meters). For a raw `point` the fields are inline. */
function endpointCoords(ep: MeasureEndpoint): Coordinates {
  return ep.kind === 'point' ? { x: ep.x, y: ep.y, z: ep.z } : ep.coords;
}

/** Stable identity for ruler state: a distinct project file - even a duplicate
 *  sharing created_at/modified_at - and any edit (modified_at) yield a different
 *  key, so an in-flight along-line reply can't bleed across them (#26). */
function projectIdentityKey(
  project: { created_at?: string; modified_at?: string; file_path?: string | null } | null | undefined
): string {
  return `${project?.created_at ?? ''}|${project?.modified_at ?? ''}|${project?.file_path ?? ''}`;
}

/** Only stations and shot-points have a cave-line position (enable along-line + tint). */
function isConnectable(ep: MeasureEndpoint): boolean {
  return ep.kind !== 'point';
}

/** Map a UI measure endpoint to the backend `RulerEndpoint` (null for raw points). */
function toRulerEndpoint(ep: MeasureEndpoint): RulerEndpoint | null {
  if (ep.kind === 'station') {
    return { section_id: ep.section_id, station_index: ep.station_index, shot_id: null, t: null };
  }
  if (ep.kind === 'shotPoint') {
    return { section_id: ep.section_id, shot_id: ep.shot_id, t: ep.t, station_index: null };
  }
  return null;
}

interface MoveDragStartData {
  type: 'station' | 'segment' | 'shot';
  sectionId?: number;
  stationRef?: StationRef;
  shotIndex?: number;
  /** Screen coordinates of the click that initiated the drag */
  screenX?: number;
  screenY?: number;
}

interface MapToolsOptions {
  onMoveDragStart?: (data: MoveDragStartData) => void;
  onFeatureHover?: (isHovering: boolean) => void;
  /** Station keys ("sectionId:stationIndex") that cannot be moved (zero-datum or transitively connected) */
  immovableStationKeys?: Set<string>;
  /** GPS origin for converting a raw map click to meters (ruler `point` endpoints). */
  gpsOrigin?: GpsOrigin | null;
  /** Re-query the measure snap (station, else shot-point) at a screen point, fresh,
   *  respecting snap config + layer visibility. Used at click time so the endpoint
   *  matches the actual click rather than the RAF/deferred snap store (#26). */
  querySnapAtPoint?: (
    screenX: number,
    screenY: number
  ) =>
    | { kind: 'station'; station: StationRef }
    | { kind: 'shot'; section_id: SectionId; shot_id: ShotId; t: number }
    | null;
}

/**
 * Pick the nearest reachable station to a point (in metres) - the Plan-mode "set a turnaround
 * anywhere" resolver (#236). Restricts to `reachable` station keys when provided, so the pick stays
 * meaningful (an unreachable endpoint would fall back to the deepest point in the engine); with no
 * field loaded yet (`reachable === null`) any station is eligible. Returns null when none match.
 */
export function nearestReachableStationRef(
  point: { x: number; y: number },
  stations: ReadonlyArray<{ station_ref: StationRef; coordinates: { x: number; y: number } }>,
  reachable: ReadonlySet<string> | null
): StationRef | null {
  let best: StationRef | null = null;
  let bestDist = Infinity;
  for (const st of stations) {
    if (
      reachable &&
      !reachable.has(`${st.station_ref.section_id}:${st.station_ref.station_index}`)
    ) {
      continue;
    }
    const dx = st.coordinates.x - point.x;
    const dy = st.coordinates.y - point.y;
    const d = dx * dx + dy * dy;
    if (d < bestDist) {
      bestDist = d;
      best = st.station_ref;
    }
  }
  return best;
}

/**
 * The uiStore keys this hook reads (#430). Explicit, so the subscription costs a
 * 25-key shallow compare instead of a ~180-key one and can actually bail out while
 * the camera moves. The hover keys (hoveredStation / hoveredShot / hoveredSectionId)
 * are deliberately ABSENT: this hook runs inside MapCanvas, so subscribing to them
 * would re-render that whole component on every mousemove. measureSnapShot is absent
 * for the same reason - it is rewritten on every mousemove over a shot (the live `t`)
 * and is only ever read via getState() at click time (#26).
 */
const MAP_TOOLS_UI_KEYS = [
  'activeTool',
  'setSelection',
  'addToSelection',
  'toggleSelection',
  'clearSelection',
  'mergeStationA',
  'setMergeStationA',
  'setMergeStationB',
  'clearMergeStations',
  'setMeasureStart',
  'setMeasureEnd',
  'setMeasureDirect',
  'setMeasureAlongLine',
  'setMeasureAlongLinePending',
  'clearMeasure',
  'setHoveredStation',
  'setHoveredShot',
  'openDialog',
  'snappedStation',
  'showStations',
  'showShots',
  'showToast',
  'setDistanceGradientOrigin',
  'appMode',
] as const satisfies readonly (keyof UIStoreState)[];

const selectMapToolsUI = (s: UIStoreState) => pickUIKeys(s, MAP_TOOLS_UI_KEYS);

export function useMapTools(options: MapToolsOptions = {}): SurveyLayerCallbacks {
  const { t } = useTranslation();
  // Performance: subscribe to an EXPLICIT key list (MAP_TOOLS_UI_KEYS above), which
  // excludes the hover values - this hook is called from MapCanvas and would
  // otherwise cause full re-renders on every mousemove.
  const {
    activeTool,
    setSelection,
    addToSelection,
    toggleSelection,
    clearSelection,
    mergeStationA,
    setMergeStationA,
    setMergeStationB,
    clearMergeStations,
    setMeasureStart,
    setMeasureEnd,
    setMeasureDirect,
    setMeasureAlongLine,
    setMeasureAlongLinePending,
    clearMeasure,
    setHoveredStation,
    setHoveredShot,
    openDialog,
    snappedStation,
    showStations,
    showShots,
    showToast,
    setDistanceGradientOrigin,
    appMode,
  } = useUIStore(useShallow(selectMapToolsUI));
  // The painted distance overlay, including one picked during a Warnings-tab visit.
  const showDistanceGradient = useUIStore(selectDistanceOverlayActive);

  // Gas origin-pick on map click belongs to the Plan workspace mode (former showGasPlan).
  const showGasPlan = appMode === 'plan';

  const { stations, project, checkMergeConnectivity } = useProjectStore();
  // Coordinate lookups for read-only inspection (the measure tool) follow the
  // active geometry so distances match what's drawn; editing/comment paths below
  // keep using the measured `stations`.
  const activeStations = useActiveStations();

  const [isCheckingConnectivity, setIsCheckingConnectivity] = useState(false);
  // Ref-based guard prevents re-entrancy from fast successive clicks
  const connectivityInFlightRef = useRef(false);
  // Monotonic request ID for staleness detection
  const connectivityRequestIdRef = useRef(0);

  // Get station coordinates by ref
  const getStationCoordinates = useCallback(
    (ref: StationRef): Coordinates | null => {
      const station = activeStations.find(
        (s) => s.station_ref.section_id === ref.section_id && s.station_ref.station_index === ref.station_index
      );
      return station?.coordinates ?? null;
    },
    [activeStations]
  );

  // Direct (3D straight-line) distance + depth diff between two measure endpoints.
  // A raw map `point` has no real depth (its z is a sentinel 0): when measuring
  // between a point and a real station/shot we treat the point's depth as equal to
  // the other endpoint's, so the readout reports the horizontal distance with zero
  // depth change rather than inflating it by the geometry endpoint's depth (#26).
  // Two raw points likewise have no depth change.
  const calculateDistance = useCallback(
    (a: MeasureEndpoint, b: MeasureEndpoint): { distance: number; depthDiff: number } => {
      const ca = endpointCoords(a);
      const cb = endpointCoords(b);
      const aHasDepth = a.kind !== 'point';
      const bHasDepth = b.kind !== 'point';
      const za = aHasDepth ? ca.z : bHasDepth ? cb.z : 0;
      const zb = bHasDepth ? cb.z : aHasDepth ? ca.z : 0;
      return {
        distance: distance({ x: ca.x, y: ca.y, z: za }, { x: cb.x, y: cb.y, z: zb }),
        depthDiff: zb - za,
      };
    },
    []
  );

  // Build a shot-point endpoint from a shot id + fractional position `t`,
  // interpolating the 3D coords from the active stations (geometry-mode aware).
  const buildShotPointEndpoint = useCallback(
    (sectionId: SectionId, shotId: ShotId, t: number): MeasureEndpoint | null => {
      const section = project?.sections.find((s) => s.id === sectionId);
      if (!section) return null;
      const pos = section.shots.findIndex((sh) => sh.id === shotId);
      if (pos < 0) return null;
      const from = getStationCoordinates({ section_id: sectionId, station_index: pos });
      const to = getStationCoordinates({ section_id: sectionId, station_index: pos + 1 });
      if (!from || !to) return null;
      const coords: Coordinates = {
        x: from.x + t * (to.x - from.x),
        y: from.y + t * (to.y - from.y),
        z: from.z + t * (to.z - from.z),
      };
      return { kind: 'shotPoint', section_id: sectionId, shot_id: shotId, t, coords };
    },
    [project?.sections, getStationCoordinates]
  );

  // Fractional position `t` of a click along a shot, derived from the click's map
  // position projected onto the shot segment. The plan view is affine, so this
  // meters-space projection matches the on-screen one. Used when no live snap is
  // available (snapping disabled), so a click near an end of a long shot isn't
  // recorded as the midpoint (#26, Codex).
  const clickShotParam = useCallback(
    (sectionId: SectionId, shotId: ShotId, event: MapMouseEvent): number | null => {
      if (!event.lngLat) return null;
      const section = project?.sections.find((s) => s.id === sectionId);
      if (!section) return null;
      const pos = section.shots.findIndex((sh) => sh.id === shotId);
      if (pos < 0) return null;
      const from = getStationCoordinates({ section_id: sectionId, station_index: pos });
      const to = getStationCoordinates({ section_id: sectionId, station_index: pos + 1 });
      if (!from || !to) return null;
      const m = mapCoordsToMeters(event.lngLat.lng, event.lngLat.lat, options.gpsOrigin ?? null);
      return projectParamOnSegment(m.x, m.y, from.x, from.y, to.x, to.y);
    },
    [project?.sections, getStationCoordinates, options.gpsOrigin]
  );

  // Monotonic id so a fast second measurement discards a stale along-line response.
  const measureRequestIdRef = useRef(0);

  // Query the backend for the along-cave-line distance + path shots, then store
  // the result + tint ids (or null on disconnect / non-station endpoints).
  const requestAlongLine = useCallback(
    async (start: MeasureEndpoint, end: MeasureEndpoint) => {
      const a = toRulerEndpoint(start);
      const b = toRulerEndpoint(end);
      if (!a || !b) {
        setMeasureAlongLine({ alongLine: null, pathShots: [], pathStations: [] });
        return;
      }
      measureRequestIdRef.current += 1;
      const requestId = measureRequestIdRef.current;
      // Capture the project identity so a response landing after the project
      // changed is discarded directly, without depending on the MapCanvas reset
      // effects having already run (they null the endpoints, but only after the
      // change's render commits). Includes modified_at so an edit/undo/redo to the
      // SAME project also invalidates the request, and file_path so opening a
      // distinct file that happens to share created_at/modified_at (e.g. a
      // duplicated project) can't inherit an in-flight reply (#26, Codex).
      const requestProjectKey = projectIdentityKey(useProjectStore.getState().project);
      setMeasureAlongLinePending(true);
      try {
        const result = await invokeIpc<AlongLineResult | null>(
          'get_along_line_distance',
          { a, b },
          { purpose: 'ruler along-line distance' }
        );
        // Discard the response unless the EXACT measurement it was issued for is
        // still live. We compare the live endpoints by reference against the ones
        // this request was made for: every new click builds fresh endpoint objects,
        // so a reset/restart - or a different completed measurement that never calls
        // requestAlongLine (e.g. a non-connectable station→raw-point one, which
        // leaves this hook-local request ref untouched) - fails the check. Without
        // it, a late reply would resurrect a stale readout/path tint over an
        // unrelated measurement (#26, Codex). The project check additionally guards
        // a switch landing before the MapCanvas clearMeasure effect runs.
        const ui = useUIStore.getState();
        if (requestId !== measureRequestIdRef.current) return;
        if (ui.activeTool !== 'measure' || ui.measureStart !== start || ui.measureEnd !== end) return;
        if (projectIdentityKey(useProjectStore.getState().project) !== requestProjectKey) return;
        setMeasureAlongLine(
          result
            ? { alongLine: result.along_line, pathShots: result.path_shots, pathStations: result.path_stations }
            : { alongLine: null, pathShots: [], pathStations: [] }
        );
      } catch (e) {
        if (requestId === measureRequestIdRef.current) {
          console.warn('Along-line distance query failed:', getErrorMessage(e, 'unknown error'));
          setMeasureAlongLine({ alongLine: null, pathShots: [], pathStations: [] });
        }
      }
    },
    [setMeasureAlongLine, setMeasureAlongLinePending]
  );

  // Apply a measure click: first point starts; second finishes (direct distance +
  // along-line query); a click on a completed measurement resets and restarts (#26).
  const applyMeasureClick = useCallback(
    (ep: MeasureEndpoint) => {
      const { measureStart: start, measureEnd: end } = useUIStore.getState();
      if (start && end) {
        clearMeasure();
        setMeasureStart(ep);
        return;
      }
      if (!start) {
        setMeasureStart(ep);
        return;
      }
      setMeasureEnd(ep);
      setMeasureDirect(calculateDistance(start, ep));
      if (isConnectable(start) && isConnectable(ep)) {
        void requestAlongLine(start, ep);
      } else {
        setMeasureAlongLine({ alongLine: null, pathShots: [], pathStations: [] });
      }
    },
    [calculateDistance, clearMeasure, requestAlongLine, setMeasureAlongLine, setMeasureDirect, setMeasureEnd, setMeasureStart]
  );

  // Check connectivity and open appropriate merge/closure dialog
  const checkAndOpenMergeDialog = useCallback(
    async (stationA: StationRef, stationB: StationRef) => {
      // Immediate ref-based guard prevents re-entrancy from fast clicks
      if (connectivityInFlightRef.current) return;
      connectivityInFlightRef.current = true;

      // Staleness: capture request ID so we can discard stale results
      connectivityRequestIdRef.current += 1;
      const requestId = connectivityRequestIdRef.current;

      setIsCheckingConnectivity(true);
      setMergeStationB(stationB);
      showToast(t('map.toasts.mergeChecking'), TOAST_KEYS.MERGE_CHECK, 'info');
      try {
        const result = await checkMergeConnectivity(stationA, stationB);
        // Discard result if a newer request was started or merge was cancelled
        if (requestId !== connectivityRequestIdRef.current) return;
        // Verify merge tool is still active (user may have switched tools)
        const currentTool = useUIStore.getState().activeTool;
        const currentA = useUIStore.getState().mergeStationA;
        if (currentTool !== 'merge' || !currentA) {
          clearMergeStations();
          return;
        }
        if (result.can_merge) {
          // Merge is allowed - open merge dialog
          setSource('map');
          openDialog('merge', {
            stationA,
            stationB,
          });
        } else if (result.connected || result.reason === 'Both sections have fixed coordinates') {
          // Connected (or both sections fixed) - redirect to closure dialog
          setSource('map');
          openDialog('closure', {
            stationA,
            stationB,
            preview: result.preview,
            reason: result.reason,
            closureRefusal: result.closure_refusal,
          });
        } else {
          // Not connected and merge blocked (e.g. both sides have zero datums)
          showToast(result.reason ?? t('map.toasts.mergeNotPossible'), TOAST_KEYS.MERGE_BLOCKED, 'warning');
          clearMergeStations();
        }
      } catch (e) {
        if (requestId !== connectivityRequestIdRef.current) return;
        showToast(getErrorMessage(e), TOAST_KEYS.MERGE_ERROR, 'error');
        clearMergeStations();
      } finally {
        if (requestId === connectivityRequestIdRef.current) {
          connectivityInFlightRef.current = false;
          setIsCheckingConnectivity(false);
        }
      }
    },
    [checkMergeConnectivity, clearMergeStations, openDialog, setMergeStationB, showToast]
  );

  // Handle station click based on active tool
  const handleStationClick = useCallback(
    async (ref: StationRef, event: { originalEvent: MouseEvent }) => {
      // GUARD: No interactions for pan/zoom tools
      if (isNonInteractiveTool(activeTool)) return;

      // GUARD: Block all station interactions when hidden
      if (!showStations) {
        showToast(t('map.toasts.stationsHidden'), TOAST_KEYS.STATIONS_HIDDEN, 'warning');
        return;
      }

      const mouseEvent = event.originalEvent;
      setSource('map');

      switch (activeTool) {
        case 'select': {
          const sel: Selection = { type: 'station', stationRef: ref };
          if (mouseEvent.shiftKey) {
            addToSelection(sel);
          } else if (mouseEvent.ctrlKey || mouseEvent.metaKey) {
            toggleSelection(sel);
          } else {
            setSelection(sel);
          }
          // Distance gradient (#34): a plain select-click re-roots the overlay to
          // the clicked station (additive - selection still happens).
          if (showDistanceGradient && !mouseEvent.shiftKey && !mouseEvent.ctrlKey && !mouseEvent.metaKey) {
            setDistanceGradientOrigin(ref);
          }
          // Gas planning (#298, #395): a plain select-click sets the route ENDPOINT once an
          // origin exists (or the "Pick on map" picker is armed, which always sets the origin),
          // and sets the ORIGIN itself before one is chosen - see `applyGasPlanClick`. Additive:
          // selection still happens. Mutually exclusive with the distance overlay, which keeps
          // re-rooting on click.
          if (showGasPlan && !mouseEvent.shiftKey && !mouseEvent.ctrlKey && !mouseEvent.metaKey) {
            applyGasPlanClick(ref);
          }
          break;
        }

        case 'merge': {
          if (isCheckingConnectivity) break;
          if (!mergeStationA) {
            setMergeStationA(ref);
          } else {
            // Check if clicking the same station
            if (
              mergeStationA.section_id === ref.section_id &&
              mergeStationA.station_index === ref.station_index
            ) {
              // Clear selection
              clearMergeStations();
            } else {
              // Check connectivity and open appropriate dialog
              checkAndOpenMergeDialog(mergeStationA, ref);
            }
          }
          break;
        }

        case 'measure': {
          const coords = getStationCoordinates(ref);
          if (coords) {
            applyMeasureClick({
              kind: 'station',
              section_id: ref.section_id,
              station_index: ref.station_index,
              coords,
            });
          }
          break;
        }

        case 'move_station': {
          // Guard: block interaction with immovable stations (zero-datum or transitively connected)
          const stationKey = `${ref.section_id}:${ref.station_index}`;
          if (options.immovableStationKeys?.has(stationKey)) break;

          // Start drag operation - handled by MapCanvas
          setSelection({ type: 'station', stationRef: ref });
          options.onMoveDragStart?.({
            type: 'station',
            stationRef: ref,
            screenX: mouseEvent.clientX,
            screenY: mouseEvent.clientY,
          });
          break;
        }


        case 'move_segment': {
          // Start segment move drag - anchored sections are silently ignored
          options.onMoveDragStart?.({
            type: 'segment',
            sectionId: ref.section_id,
            stationRef: ref,
            screenX: mouseEvent.clientX,
            screenY: mouseEvent.clientY,
          });
          break;
        }

        case 'comment': {
          setSelection({ type: 'station', stationRef: ref });
          // Lookup the station's current comment
          const station = stations.find(
            (s) => s.station_ref.section_id === ref.section_id && s.station_ref.station_index === ref.station_index
          );
          const currentComment = station?.comment ?? '';
          useUIStore.getState().setCommentTarget({
            type: 'station',
            stationRef: ref,
            screenPos: { x: mouseEvent.clientX, y: mouseEvent.clientY },
            currentComment,
          });
          break;
        }

        case 'move_comment': {
          const { moveCommentSource, setMoveCommentSource, clearMoveCommentSource } = useUIStore.getState();
          const mcStation = stations.find(
            (s) => s.station_ref.section_id === ref.section_id && s.station_ref.station_index === ref.station_index
          );
          if (!moveCommentSource) {
            // First click: pick up source - just record it, don't mutate backend
            if (!mcStation?.comment) {
              showToast(t('map.toasts.noCommentToMove'), TOAST_KEYS.MOVE_COMMENT_EMPTY, 'warning');
              break;
            }
            setSelection({ type: 'station', stationRef: ref });
            setMoveCommentSource({ type: 'station', stationRef: ref, comment: mcStation.comment, screenPos: { x: mouseEvent.clientX, y: mouseEvent.clientY } });
          } else {
            // Second click: target - use atomic move_comment IPC
            const targetComment = mcStation?.comment ?? '';
            if (targetComment) {
              // Target has comment - open swap dialog
              openDialog('swap-comments', {
                source: moveCommentSource,
                target: { type: 'station', stationRef: ref, comment: targetComment },
              });
            } else {
              // Target empty - atomic move
              const source = moveCommentSource.type === 'station' && moveCommentSource.stationRef
                ? { type: 'station' as const, station_ref: moveCommentSource.stationRef }
                : { type: 'shot' as const, section_id: moveCommentSource.sectionId!, shot_id: moveCommentSource.shotId! };
              const target = { type: 'station' as const, station_ref: ref };
              await useProjectStore.getState().moveComment(source, target, false);
              clearMoveCommentSource();
            }
          }
          break;
        }
      }
    },
    [
      activeTool,
      addToSelection,
      applyMeasureClick,
      getStationCoordinates,
      checkAndOpenMergeDialog,
      clearMergeStations,
      isCheckingConnectivity,
      mergeStationA,
      openDialog,
      // The individual fields this handler reads, not the `options` object: the
      // caller builds that object fresh every render, so depending on it would
      // rebuild the handler (and the memoized callback bundle below) each time (#430).
      options.immovableStationKeys,
      options.onMoveDragStart,
      project?.sections,
      setMergeStationA,
      setSelection,
      showStations,
      showToast,
      stations,
      toggleSelection,
      showDistanceGradient,
      setDistanceGradientOrigin,
      showGasPlan,
    ]
  );

  // Handle shot click based on active tool
  const handleShotClick = useCallback(
    async (sectionId: SectionId, shotId: ShotId, event: MapMouseEvent) => {
      // GUARD: No interactions for pan/zoom tools
      if (isNonInteractiveTool(activeTool)) return;

      // GUARD: Block all shot interactions when hidden
      if (!showShots) {
        showToast(t('map.toasts.shotsHidden'), TOAST_KEYS.SHOTS_HIDDEN, 'warning');
        return;
      }

      const mouseEvent = event.originalEvent;
      setSource('map');

      switch (activeTool) {
        case 'select': {
          const sel: Selection = { type: 'shot', sectionId, shotId };
          if (mouseEvent.shiftKey) {
            addToSelection(sel);
          } else if (mouseEvent.ctrlKey || mouseEvent.metaKey) {
            toggleSelection(sel);
          } else {
            setSelection(sel);
          }
          // Distance gradient (#34): re-root on a shot click too. With stations
          // hidden - common (and nicer-looking) while the gradient is on - you
          // can't click a station, so re-root to the shot's lower-numbered
          // endpoint station (section_id, shotIndex).
          if (
            (showDistanceGradient || showGasPlan) &&
            !mouseEvent.shiftKey &&
            !mouseEvent.ctrlKey &&
            !mouseEvent.metaKey
          ) {
            const section = project?.sections.find((s) => s.id === sectionId);
            const shotIndex = section?.shots.findIndex((s) => s.id === shotId) ?? -1;
            if (shotIndex !== -1) {
              const ref = { section_id: sectionId, station_index: shotIndex };
              // Mutually exclusive overlays: the distance gradient re-roots, Plan applies the
              // same origin/endpoint split as the station click above (#298, #395).
              if (showDistanceGradient) setDistanceGradientOrigin(ref);
              if (showGasPlan) {
                // A shot click anchors to whichever of its two endpoint stations is nearer the
                // current origin, not always the lower-numbered one (#395 regression fix) -
                // "current origin" is the entrance pre-origin, or the outgoing origin on a
                // re-armed pick-on-map (see nearestOriginShotEndpoint's JSDoc).
                const other = { section_id: sectionId, station_index: shotIndex + 1 };
                const nearer = nearestOriginShotEndpoint(
                  ref,
                  other,
                  useGasPlanStore.getState().reachField,
                  activeStations
                );
                applyGasPlanClick(nearer);
              }
            }
          }
          break;
        }

        case 'move_shot': {
          // Guard: block interaction if either shot endpoint is immovable
          if (options.immovableStationKeys) {
            const section = project?.sections.find((s) => s.id === sectionId);
            if (section) {
              const shotIndex = section.shots.findIndex((s) => s.id === shotId);
              if (shotIndex !== -1) {
                const fromKey = `${sectionId}:${shotIndex}`;
                const toKey = `${sectionId}:${shotIndex + 1}`;
                if (options.immovableStationKeys.has(fromKey) || options.immovableStationKeys.has(toKey)) break;
              }
            }
          }

          // Select the shot and clear other selection effects
          setSelection({ type: 'shot', sectionId, shotId });
          // Find the shot index within the section
          const shotSection = project?.sections.find((s) => s.id === sectionId);
          if (shotSection) {
            const shotIndex = shotSection.shots.findIndex((s) => s.id === shotId);
            if (shotIndex !== -1) {
              options.onMoveDragStart?.({
                type: 'shot',
                sectionId,
                shotIndex,
                screenX: mouseEvent.clientX,
                screenY: mouseEvent.clientY,
              });
            }
          }
          break;
        }

        case 'move_segment': {
          // Start segment move drag from a shot click (same as station click)
          options.onMoveDragStart?.({
            type: 'segment',
            sectionId,
            screenX: mouseEvent.clientX,
            screenY: mouseEvent.clientY,
          });
          break;
        }

        case 'comment': {
          setSelection({ type: 'shot', sectionId, shotId });
          // Lookup the shot's current comment
          const section = project?.sections.find((s) => s.id === sectionId);
          const shot = section?.shots.find((s) => s.id === shotId);
          const currentComment = shot?.comment ?? '';
          useUIStore.getState().setCommentTarget({
            type: 'shot',
            sectionId,
            shotId,
            screenPos: { x: mouseEvent.clientX, y: mouseEvent.clientY },
            currentComment,
          });
          break;
        }

        case 'move_comment': {
          const { moveCommentSource, setMoveCommentSource, clearMoveCommentSource } = useUIStore.getState();
          const mcSection = project?.sections.find((s) => s.id === sectionId);
          const mcShot = mcSection?.shots.find((s) => s.id === shotId);
          if (!moveCommentSource) {
            // First click: pick up source - just record it, don't mutate backend
            if (!mcShot?.comment) {
              showToast(t('map.toasts.noCommentToMove'), TOAST_KEYS.MOVE_COMMENT_EMPTY, 'warning');
              break;
            }
            setSelection({ type: 'shot', sectionId, shotId });
            setMoveCommentSource({ type: 'shot', sectionId, shotId, comment: mcShot.comment, screenPos: { x: mouseEvent.clientX, y: mouseEvent.clientY } });
          } else {
            // Second click: target - use atomic move_comment IPC
            const targetComment = mcShot?.comment ?? '';
            if (targetComment) {
              // Target has comment - open swap dialog
              openDialog('swap-comments', {
                source: moveCommentSource,
                target: { type: 'shot', sectionId, shotId, comment: targetComment },
              });
            } else {
              // Target empty - atomic move
              const source = moveCommentSource.type === 'station' && moveCommentSource.stationRef
                ? { type: 'station' as const, station_ref: moveCommentSource.stationRef }
                : { type: 'shot' as const, section_id: moveCommentSource.sectionId!, shot_id: moveCommentSource.shotId! };
              const target = { type: 'shot' as const, section_id: sectionId, shot_id: shotId };
              await useProjectStore.getState().moveComment(source, target, false);
              clearMoveCommentSource();
            }
          }
          break;
        }

        case 'measure': {
          // A station within the snap threshold wins even when the click landed on a
          // shot's hitbox (the snap threshold can exceed the station hit area), so
          // re-check the fresh snap first (#26, Codex).
          const snap = options.querySnapAtPoint?.(event.point.x, event.point.y) ?? null;
          if (snap?.kind === 'station') {
            const coords = getStationCoordinates(snap.station);
            if (coords) {
              applyMeasureClick({
                kind: 'station',
                section_id: snap.station.section_id,
                station_index: snap.station.station_index,
                coords,
              });
              break;
            }
          }
          // Otherwise derive t directly from the click position on this shot, rather
          // than the RAF/deferred snap store (which can hold a stale shot/t), falling
          // back to the midpoint only if it can't be projected.
          const t = clickShotParam(sectionId, shotId, event) ?? 0.5;
          const ep = buildShotPointEndpoint(sectionId, shotId, t);
          if (ep) applyMeasureClick(ep);
          break;
        }
      }
    },
    // See handleStationClick: the read `options` fields, not the object (#430).
    [activeTool, activeStations, addToSelection, applyMeasureClick, buildShotPointEndpoint, clickShotParam, openDialog, options.immovableStationKeys, options.onMoveDragStart, options.querySnapAtPoint, project?.sections, setSelection, showShots, showToast, toggleSelection, showDistanceGradient, setDistanceGradientOrigin, showGasPlan]
  );

  // Handle empty click (no feature) - but use snapped station if available
  const handleEmptyClick = useCallback(
    (event: MapMouseEvent) => {
      // Measure: commit the current snap (shot-point or station) or, failing that,
      // a raw map point. Handled before the snapped-station branch so a shot snap
      // (no snapped station) works, and so the reset-and-restart on a fresh click
      // applies regardless of snap state (#26).
      if (activeTool === 'measure') {
        let ep: MeasureEndpoint | null = null;
        // Re-derive the snap from the ACTUAL click position rather than the
        // RAF/deferred snap store, which can still hold the previous shot/t if the
        // cursor moved or left the shot just before the click. The fresh query keeps
        // the "stations win" precedence and respects snap config + layer visibility
        // (a hidden / hidden-reference shot can't snap) (#26, Codex).
        const snap = options.querySnapAtPoint?.(event.point.x, event.point.y) ?? null;
        if (snap?.kind === 'station') {
          const coords = getStationCoordinates(snap.station);
          if (coords) {
            ep = { kind: 'station', section_id: snap.station.section_id, station_index: snap.station.station_index, coords };
          }
        } else if (snap?.kind === 'shot') {
          ep = buildShotPointEndpoint(snap.section_id, snap.shot_id, snap.t);
        }
        if (!ep && event.lngLat) {
          const m = mapCoordsToMeters(event.lngLat.lng, event.lngLat.lat, options.gpsOrigin ?? null);
          ep = { kind: 'point', x: m.x, y: m.y, z: 0 };
        }
        if (ep) applyMeasureClick(ep);
        return;
      }

      // If we have a snapped station, use it instead of treating as empty click
      // This enables snap functionality for merge/measure tools
      if (snappedStation) {
        switch (activeTool) {
          case 'merge':
            // GUARD: Block merge snap when stations are hidden
            if (!showStations) {
              showToast(t('map.toasts.stationsHidden'), TOAST_KEYS.STATIONS_HIDDEN, 'warning');
              return;
            }
            if (isCheckingConnectivity) return;
            if (!mergeStationA) {
              setMergeStationA(snappedStation);
            } else {
              // Check if clicking the same station
              if (
                mergeStationA.section_id === snappedStation.section_id &&
                mergeStationA.station_index === snappedStation.station_index
              ) {
                clearMergeStations();
              } else {
                // Check connectivity and open appropriate dialog
                checkAndOpenMergeDialog(mergeStationA, snappedStation);
              }
            }
            return;
        }
      }

      // No snapped station, handle as regular empty click
      switch (activeTool) {
        case 'select':
        case 'move_segment':
          clearSelection();
          break;

        case 'comment':
          useUIStore.getState().clearCommentTarget();
          break;

        case 'move_comment': {
          // Cancel: no backend mutation happened, just clear UI state
          useUIStore.getState().clearMoveCommentSource();
          break;
        }

        case 'merge':
          // Don't clear during connectivity check - the staleness guard handles cancellation
          if (!isCheckingConnectivity) {
            clearMergeStations();
          }
          break;
      }
    },
    [
      activeTool,
      applyMeasureClick,
      buildShotPointEndpoint,
      getStationCoordinates,
      checkAndOpenMergeDialog,
      snappedStation,
      isCheckingConnectivity,
      mergeStationA,
      clearMergeStations,
      clearSelection,
      openDialog,
      // See handleStationClick: the read `options` fields, not the object (#430).
      options.gpsOrigin,
      options.querySnapAtPoint,
      setMergeStationA,
      showShots,
      showStations,
      showToast,
    ]
  );

  // Handle station hover - only highlights the specific station, not the whole section.
  // Panel-side section-name highlight is derived in SectionsPanel from the hovered
  // shot/station, so we deliberately don't touch hoveredSectionId here (doing so would
  // (a) feed back into SurveyLayer.setHoveredSection and light up the whole section on
  // the map, and (b) get clobbered by the trailing null callback when the cursor moves
  // from a shot to a station).
  const handleStationHover = useCallback(
    (ref: StationRef | null) => {
      // GUARD: No hover for pan/zoom tools
      if (isNonInteractiveTool(activeTool)) {
        setHoveredStation(null);
        return;
      }
      setHoveredStation(ref);
    },
    [activeTool, setHoveredStation]
  );

  // Handle shot hover - only highlights the specific shot, not the whole section.
  // See handleStationHover for why we don't set hoveredSectionId here.
  const handleShotHover = useCallback(
    (shot: { sectionId: SectionId; shotId: ShotId } | null) => {
      // GUARD: No hover for pan/zoom tools
      if (isNonInteractiveTool(activeTool)) {
        setHoveredShot(null);
        return;
      }
      setHoveredShot(shot);
    },
    [activeTool, setHoveredShot]
  );

  // Handle context menu
  const handleContextMenu = useCallback(
    (
      target:
        | { type: 'station'; ref: StationRef }
        | { type: 'shot'; sectionId: SectionId; shotId: ShotId; alongT: number }
        | null,
      event: MapMouseEvent
    ) => {
      // ALWAYS prevent native context menu
      event.originalEvent.preventDefault();

      // GUARD: No context menu for pan/zoom tools
      if (isNonInteractiveTool(activeTool)) return;

      // Plan (gas) mode: the only context action is picking a route point. A station target uses
      // that station (handled below); a SHOT target or an empty click snaps to the nearest reachable
      // station - so right-clicking the prominent gas-coloured shot lines (very likely, since
      // stations are often hidden), or empty cave when the whole map is reachable (low RMV / lots of
      // gas → no turn-frontier dots), still opens the route menu instead of silently doing nothing.
      // Match in metres against the click's map coords; restrict to reachable stations (when the
      // field is loaded) so the pick is meaningful - an unreachable endpoint would fall back to the
      // deepest point in the engine. The ContextMenu Plan branch turns it into Set as endpoint. (#236)
      if (showGasPlan && target?.type !== 'station') {
        const reachField = useGasPlanStore.getState().reachField;
        const clickM = mapCoordsToMeters(event.lngLat.lng, event.lngLat.lat, options.gpsOrigin ?? null);
        const reachable = reachField ? reachableStationKeySet(reachField) : null;
        // Prefer a reachable station, but fall back to the nearest station of ANY kind when none
        // is in reach - otherwise the menu simply would not open, and with way points able to
        // make the whole map unreachable (#298) the user would be locked out of the very menu
        // items ("Set as start point", removing a way point) that fix it.
        const best =
          nearestReachableStationRef(clickM, activeStations, reachable) ??
          nearestReachableStationRef(clickM, activeStations, null);
        if (best) {
          setSource('map');
          setSelection({ type: 'station', stationRef: best });
          openDialog('context-menu', {
            // Over the map canvas: no scroller can move it, and opening it sets
            // a selection that scrolls the panels. This is the station/shot
            // right-click the flash-and-vanish bug was reported against.
            survivesScroll: true,
            target: { type: 'station', ref: best },
            position: { x: event.originalEvent.clientX, y: event.originalEvent.clientY },
          });
        }
        return;
      }

      if (target) {
        setSource('map');
        // Select the target first
        if (target.type === 'station') {
          setSelection({ type: 'station', stationRef: target.ref });
        } else {
          setSelection({ type: 'shot', sectionId: target.sectionId, shotId: target.shotId });
        }

        // Open context menu
        openDialog('context-menu', {
          // Map canvas - see above.
          survivesScroll: true,
          target,
          position: { x: event.originalEvent.clientX, y: event.originalEvent.clientY },
        });
      }
    },
    [activeTool, openDialog, setSelection, showGasPlan, activeStations, options.gpsOrigin]
  );

  // Memoized: a fresh object literal here churned `toolCallbacks` in MapCanvas on
  // every render, which re-ran the `SurveyLayer.setCallbacks` effect each time (#430).
  // Every member is now render-stable (the handlers are useCallback'd over narrowed
  // deps; onFeatureHover is the caller's own callback), so this rebuilds only when a
  // handler genuinely changes.
  return useMemo<SurveyLayerCallbacks>(
    () => ({
      onStationClick: handleStationClick,
      onShotClick: handleShotClick,
      onEmptyClick: handleEmptyClick,
      onStationHover: handleStationHover,
      onShotHover: handleShotHover,
      onFeatureHover: options.onFeatureHover,
      onContextMenu: handleContextMenu,
    }),
    [
      handleStationClick,
      handleShotClick,
      handleEmptyClick,
      handleStationHover,
      handleShotHover,
      options.onFeatureHover,
      handleContextMenu,
    ]
  );
}
