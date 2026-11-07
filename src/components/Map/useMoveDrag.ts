import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type MutableRefObject,
  type RefObject,
} from 'react';
import type * as maplibregl from 'maplibre-gl';
import type { TFunction } from 'i18next';
import type { Station, StationRef } from '../../types/project';
import type { GpsOrigin } from '../../types/config';
import type { BatchCommand } from '../../types/commands';
import { useProjectStore, type ProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { UIStoreState } from '../../store/uiSelectKeys';
import { beginMoveCarry, commitMoveCarry, endMoveCarry, isMoveCarryLive } from '../../store/moveCarry';
import { getErrorMessage } from '../../utils/ipc';
import { metersToMapCoords, mapCoordsToMeters } from '../../utils/coordinates';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { refusedMergeOffersClosure } from '../../utils/mergeConnectivity';
import type SurveyLayer from './SurveyLayer';

interface MoveDragState {
  type: 'station' | 'segment' | 'shot';
  sectionId?: number;
  stationRef?: { section_id: number; station_index: number };
  shotIndex?: number;
  connectedSectionIds?: number[];
  startLngLat: { lng: number; lat: number };
  currentLngLat: { lng: number; lat: number };
}

interface MoveDragOptions {
  map: MutableRefObject<maplibregl.Map | null>;
  mapContainer: RefObject<HTMLDivElement>;
  surveyLayerRef: MutableRefObject<SurveyLayer | null>;
  /** The carry registered with src/store/moveCarry.ts; owned by MapCanvas, which also runs useMoveCarryGuards on it. */
  moveCarryTokenRef: MutableRefObject<number | null>;
  config: ProjectStore['config'];
  stations: ProjectStore['stations'];
  activeStations: Station[];
  gpsOrigin: GpsOrigin | null;
  immovableStationKeys: Set<string>;
  anchoredSectionIds: Set<number>;
  anchoredIdsLoaded: boolean;
  clearSelectionAndVisuals: () => void;
  getConnectedSectionIds: ProjectStore['getConnectedSectionIds'];
  moveStation: ProjectStore['moveStation'];
  moveSegment: ProjectStore['moveSegment'];
  executeBatch: ProjectStore['executeBatch'];
  setSnappedStation: UIStoreState['setSnappedStation'];
  t: TFunction;
}

/**
 * The move tools' pickup / drag / drop (docs/move-tools.md): the drag state and
 * its hot-path refs, the one carry cancel routine (GHOST-2 R5), the pickup
 * (`handleMoveDragStart`, handed to useMapTools), the per-move preview with
 * snap-to-merge, the map-move re-apply (#79), the snap-merge prompt, the drop
 * (`handleMoveDragEnd`) and the Escape key that cancels a carry.
 *
 * Moved out of MapCanvas.tsx verbatim (#576 D3); the carry registration,
 * refusal and stale-commit checks are untouched. MapCanvas keeps the carry token
 * (`moveCarryTokenRef`, which useMoveCarryGuards watches) and the press point
 * that click-gates the drop (`pressClientRef`), and calls this hook where
 * `cancelMoveCarryState` used to be declared, so the map-move re-apply effect
 * keeps its place in the component's effect order. The Escape effect now
 * registers its window keydown listener earlier than it did; MapCanvas's keydown
 * listeners neither stop propagation nor read `defaultPrevented`, and each acts
 * on its own key / state, so the order they run in cannot change an outcome.
 */
export function useMoveDrag({
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
}: MoveDragOptions) {
  const [moveDrag, setMoveDrag] = useState<MoveDragState | null>(null);
  // Ref mirror of `moveDrag` for the SurveyLayer callbacks, which must stay
  // render-stable (#430) and so cannot close over the state itself.
  const moveDragRef = useRef<MoveDragState | null>(null);
  useLayoutEffect(() => {
    moveDragRef.current = moveDrag;
  }, [moveDrag]);
  // Token to invalidate stale getConnectedSectionIds results (incremented on each drag start,
  // checked in .then()/.catch() to discard late async results after mouseup)
  const segmentDragTokenRef = useRef(0);
  // The token of the latest Move Segment pickup. While it still equals
  // segmentDragTokenRef that pickup is live: pending until getConnectedSectionIds
  // returns and `moveDrag` is set, then carried until a drop or Escape bumps the
  // token. The right-click gate reads it because `moveDrag` is still null while
  // the pickup is pending.
  const segmentPickupTokenRef = useRef<number | null>(null);
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

  // The ONE way a carry is cancelled (GHOST-2 R5): Escape, another mutation (R1), the
  // survey changing or editing becoming blocked (R2) and a tool switch (R4) all end
  // here. Also right in the Move Segment pickup window, where `moveDrag` is still
  // null: the token bump makes the pending connected-section fetch discard itself.
  const cancelMoveCarryState = useCallback(() => {
    const token = moveCarryTokenRef.current;
    moveCarryTokenRef.current = null;
    endMoveCarry(token);
    // Clean up drag preview (temp layers, filters, sources)
    surveyLayerRef.current?.endDragPreview();
    // A snap-merge dialog this carry opened goes with it (its Cancel would resume a
    // carry that no longer exists, its confirm would commit one).
    const ui = useUIStore.getState();
    if (token !== null && ui.dialogData.moveCarryToken === token) ui.closeDialog();
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
  }, [setSnappedStation]);

  // A pickup starts a carry against the survey generation it was picked up on, so the
  // mutation chokepoints can see it (src/store/moveCarry.ts).
  const beginCarry = useCallback(() => {
    endMoveCarry(moveCarryTokenRef.current);
    moveCarryTokenRef.current = beginMoveCarry(useProjectStore.getState().projectTreeGeneration, {
      cancel: cancelMoveCarryState,
      restorePreview: () => surveyLayerRef.current?.endDragPreview(),
    });
  }, [cancelMoveCarryState]);

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

        beginCarry();
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

        // In flight from the click, not from when the fetch below puts the ghost up.
        beginCarry();
        // Set a pending drag token to detect stale async results
        const dragToken = ++segmentDragTokenRef.current;
        segmentPickupTokenRef.current = dragToken;

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

        beginCarry();
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
    [anchoredSectionIds, anchoredIdsLoaded, beginCarry, clearSelectionAndVisuals, getConnectedSectionIds, immovableStationKeys, activeStations, gpsOrigin, config?.map?.shot?.hit_area]
  );

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
        moveCarryToken: number | null;
      }
    ) => {
      const { checkMergeConnectivity } = useProjectStore.getState();
      const { openDialog, showToast } = useUIStore.getState();

      showToast(t('map.toasts.mergeChecking'), TOAST_KEYS.MERGE_CHECK, 'info');
      try {
        const result = await checkMergeConnectivity(originStation, targetStation);
        // The carry was cancelled while the check was out (GHOST-2): no dialog for it.
        if (moveCallbacks && !isMoveCarryLive(moveCallbacks.moveCarryToken)) return;
        // The token goes along only as a number (the check above already returned
        // for a null one), so the dialogs read it as `number | undefined`.
        const carryToken = moveCallbacks?.moveCarryToken;
        const extra = moveCallbacks
          ? {
              onBeforeConfirm: moveCallbacks.onBeforeConfirm,
              onCancel: moveCallbacks.onCancel,
              preCommands: moveCallbacks.preCommands,
              ...(typeof carryToken === 'number' ? { moveCarryToken: carryToken } : {}),
            }
          : {};
        if (result.can_merge) {
          openDialog('merge', { stationA: originStation, stationB: targetStation, ...extra });
        } else if (refusedMergeOffersClosure(result)) {
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
      endMoveCarry(moveCarryTokenRef.current);
      moveCarryTokenRef.current = null;
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
          // Still in flight while the dialog is up; its confirm is this carry's commit.
          moveCarryToken: moveCarryTokenRef.current,
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
          moveCarryToken: moveCarryTokenRef.current,
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
    // The drop starts the carry's own commit; from here it is no longer in flight.
    const carryToken = moveCarryTokenRef.current;
    moveCarryTokenRef.current = null;
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
        const stationRef = moveDrag.stationRef;
        const res = await commitMoveCarry(carryToken, () =>
          colocated.length > 1
            ? executeBatch(colocated.map(ref => ({ type: 'move_station' as const, station_ref: ref, offset })))
            : moveStation(stationRef, offset)
        );
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
        const sectionId = moveDrag.sectionId;
        const res = await commitMoveCarry(carryToken, () => moveSegment(sectionId, offset));
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
        const res = await commitMoveCarry(carryToken, () =>
          executeBatch(colocated.map(ref => ({ type: 'move_station' as const, station_ref: ref, offset })))
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
    endMoveCarry(carryToken);
    stationDragColocatedRefsRef.current = [];
    if (surveyLayerRef.current) surveyLayerRef.current.endDragPreview();
    setMoveDrag(null);
  }, [moveDrag, moveStation, moveSegment, executeBatch, gpsOrigin, stations, promptSnapMerge, setSnappedStation]);

  // Escape key cancels move drag operations (station, segment, shot). Listening
  // always, not only while `moveDrag` is set: a carry is in flight from the pickup
  // click, and in the Move Segment pickup window (the connected-section fetch still
  // out) `moveDrag` is null while the carry is live.
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (!moveDrag && !isMoveCarryLive(moveCarryTokenRef.current)) return;
      e.preventDefault();
      cancelMoveCarryState();
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [moveDrag, cancelMoveCarryState]);

  return {
    moveDrag,
    moveDragRef,
    segmentDragTokenRef,
    segmentPickupTokenRef,
    snapMergeDialogPendingRef,
    handleMoveDragStart,
    handleMoveDragMove,
    handleMoveDragEnd,
  };
}
