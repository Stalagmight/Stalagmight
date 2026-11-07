import { useCallback, useEffect, useState, type MutableRefObject, type RefObject } from 'react';
import type * as maplibregl from 'maplibre-gl';
import type { GpsOrigin } from '../../types/config';
import type { ProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { UIStoreState } from '../../store/uiSelectKeys';
import { mapCoordsToMeters } from '../../utils/coordinates';
import type SurveyLayer from './SurveyLayer';
import { UNIT_PIXELS_PER_METER, type PixelsPerMeter } from './placementScale';

interface PlacementOptions {
  map: MutableRefObject<maplibregl.Map | null>;
  mapContainer: RefObject<HTMLDivElement>;
  surveyLayerRef: MutableRefObject<SurveyLayer | null>;
  config: ProjectStore['config'];
  gpsOrigin: GpsOrigin | null;
  placementActive: boolean;
  /** Shared with the map init's debug hooks and the modifier pan's click capture, so owned by MapCanvas. */
  placementJustConfirmedRef: MutableRefObject<boolean>;
  confirmPlacement: UIStoreState['confirmPlacement'];
  cancelPlacementMode: UIStoreState['cancelPlacementMode'];
  updatePlacementCursor: UIStoreState['updatePlacementCursor'];
  updatePlacementSnap: UIStoreState['updatePlacementSnap'];
  noteMapInteraction: () => void;
}

/**
 * Placement mode (DMP / new-section placement): the overlay's cursor and live
 * scale state, the station snap, confirm and cancel, the Escape key, and the
 * placement click / right-click handlers MapCanvas's pointer handlers route to.
 *
 * Moved out of MapCanvas.tsx verbatim (#576 D3). Two pieces stay in MapCanvas:
 * the placement branch of its combined pointer-move handler, and the effect that
 * seeds `pixelsPerMeter` when placement turns on - that one reads the camera, so
 * it keeps its place after the camera request effects (useMapCamera), which can
 * move the camera synchronously in the same commit. The Escape effect registers
 * its window keydown listener earlier than it did; see useMoveDrag for why the
 * order of MapCanvas's keydown listeners cannot change an outcome.
 */
export function usePlacement({
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
}: PlacementOptions) {
  // Placement mode cursor position (screen pixels)
  const [placementCursor, setPlacementCursor] = useState<{ x: number; y: number } | null>(null);
  // Current map scale in pixels per meter, east and north (for to-scale placement preview)
  const [pixelsPerMeter, setPixelsPerMeter] = useState<PixelsPerMeter>(UNIT_PIXELS_PER_METER);

  // Check snap for placement mode
  const checkPlacementSnap = useCallback(
    (screenX: number, screenY: number) => {
      if (!map.current || !config || !placementActive || !surveyLayerRef.current) {
        return null;
      }

      return surveyLayerRef.current.queryNearestStation(
        screenX, screenY, config.map.snap.threshold
      );
    },
    [config, placementActive]
  );

  // Handle placement confirmation
  const handlePlacementConfirm = useCallback(() => {
    if (!map.current || !placementActive || !mapContainer.current) return;

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
  }, [placementActive, confirmPlacement]);

  // Handle placement cancel
  const handlePlacementCancel = useCallback(() => {
    // Get the onCancel callback before canceling
    const onCancel = useUIStore.getState().placementMode?.onCancel;
    cancelPlacementMode();
    setPlacementCursor(null);
    // Call the cancel callback if provided
    if (onCancel) {
      onCancel();
    }
  }, [cancelPlacementMode]);

  // Handle Escape key for placement mode
  useEffect(() => {
    if (!placementActive) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        handlePlacementCancel();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [placementActive, handlePlacementCancel]);

  // Handle placement mode click
  const handlePlacementClick = useCallback(
    (e: React.PointerEvent) => {
      if (!placementActive || !map.current || !mapContainer.current) return;

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
    [placementActive, handlePlacementConfirm, gpsOrigin, updatePlacementCursor, updatePlacementSnap, checkPlacementSnap, noteMapInteraction]
  );

  // Handle placement mode right-click (cancel)
  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      if (placementActive) {
        e.preventDefault();
        e.stopPropagation();
        noteMapInteraction();
        handlePlacementCancel();
      }
    },
    [placementActive, handlePlacementCancel, noteMapInteraction]
  );

  return {
    placementCursor,
    setPlacementCursor,
    pixelsPerMeter,
    setPixelsPerMeter,
    checkPlacementSnap,
    handlePlacementCancel,
    handlePlacementClick,
    handleContextMenu,
  };
}
