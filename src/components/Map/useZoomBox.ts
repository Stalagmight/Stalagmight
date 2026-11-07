import { useCallback, useEffect, useState, type MutableRefObject, type RefObject } from 'react';
import type * as maplibregl from 'maplibre-gl';
import type { UIStoreState } from '../../store/uiSelectKeys';

export interface ZoomBoxState {
  start: { x: number; y: number } | null;
  end: { x: number; y: number } | null;
}

interface ZoomBoxOptions {
  map: MutableRefObject<maplibregl.Map | null>;
  mapContainer: RefObject<HTMLDivElement>;
  activeTool: UIStoreState['activeTool'];
  placementActive: boolean;
  zoomStep: number;
  zoomMin: number;
  zoomMax: number;
}

/**
 * The zoom tool: the zoom box (click-to-zoom and box zoom) with its pointer
 * handlers, the Escape key that cancels it, and the canvas capture that keeps a
 * zoom-tool click from selecting and turns a right-click into zoom-out.
 *
 * Moved out of MapCanvas.tsx verbatim (#576 D3). The box state is returned with
 * its setter because the modifier pan (useModifierPan) cancels the box when Ctrl
 * goes down or the window blurs. MapCanvas calls this right before
 * useModifierPan, so these two effects now run before the Ctrl-tracking and
 * custom-pan effects instead of between them. They cannot interact: the keydown
 * listeners act on different keys (and both only ever clear the box), and the
 * click captures sit on different elements (the container's runs first in the
 * capture phase whatever order they were added in).
 */
export function useZoomBox({
  map,
  mapContainer,
  activeTool,
  placementActive,
  zoomStep,
  zoomMin,
  zoomMax,
}: ZoomBoxOptions) {
  const [zoomBox, setZoomBox] = useState<ZoomBoxState>({ start: null, end: null });

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
      if (placementActive) return;
      if (e.button === 0 && !e.ctrlKey) {
        e.stopPropagation();
        e.stopImmediatePropagation();
        // Zoom-in is handled by handleZoomBoxEnd for <10px drags
      }
    };

    // Right-click capture: cancel zoom box if active, otherwise zoom out at click point
    const handleContextMenuCapture = (e: MouseEvent) => {
      // Don't intercept during placement mode
      if (placementActive) return;

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
  }, [activeTool, zoomStep, zoomMin, placementActive, zoomBox.start]);

  return { zoomBox, setZoomBox, handleZoomBoxStart, handleZoomBoxMove, handleZoomBoxEnd };
}
