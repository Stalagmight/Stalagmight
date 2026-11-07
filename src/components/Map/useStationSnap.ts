import { startTransition, useCallback, useEffect, useRef, type MutableRefObject } from 'react';
import * as maplibregl from 'maplibre-gl';
import type { StationRef } from '../../types/project';
import type { ProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { UIStoreState } from '../../store/uiSelectKeys';
import { isShotKindVisible } from '../../store/displaySelectors';
import type SurveyLayer from './SurveyLayer';

interface StationSnapOptions {
  map: MutableRefObject<maplibregl.Map | null>;
  surveyLayerRef: MutableRefObject<SurveyLayer | null>;
  config: ProjectStore['config'];
  project: ProjectStore['project'];
  activeTool: UIStoreState['activeTool'];
  showStations: boolean;
  showShots: boolean;
  showJumps: boolean;
  showReferenceShots: boolean;
  snappedStation: UIStoreState['snappedStation'];
  setSnappedStation: UIStoreState['setSnappedStation'];
  setMeasureSnapShot: UIStoreState['setMeasureSnapShot'];
}

/**
 * The cursor snap of the merge and measure tools (#26): the frame-rate
 * station / shot-point snap (`checkSnapToStation`, fed by MapCanvas's RAF-batched
 * pointer move), the click-time re-query handed to useMapTools
 * (`querySnapAtPoint`), and the effects that drop a stale snap when the tool
 * changes, the store snap clears, or the snapped leg is hidden.
 *
 * Moved out of MapCanvas.tsx verbatim (#576 D3). MapCanvas calls it where
 * `checkSnapToStation` used to be declared; nothing but callbacks sat between
 * that point and these effects, so they keep their place in its effect order.
 */
export function useStationSnap({
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
}: StationSnapOptions) {
  // Ref mirror of snappedStation - used for same-value comparison in the snap hot path
  // so checkSnapToStation doesn't depend on React state (avoids re-render in the loop)
  const snappedStationRef = useRef<StationRef | null>(null);
  // Ref mirror of the measure shot-snap candidate (#26) + its live cursor marker,
  // updated imperatively in the snap hot path like the station snap.
  const measureSnapShotRef = useRef<{ section_id: number; shot_id: number; t: number; lng: number; lat: number } | null>(null);
  const shotSnapMarkerRef = useRef<maplibregl.Marker | null>(null);

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

  return { checkSnapToStation, querySnapAtPoint };
}
