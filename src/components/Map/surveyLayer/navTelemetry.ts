/**
 * Navigation telemetry for the survey map (#576 D3 split): which input drove a
 * user zoom or pan. Programmatic camera moves carry no `originalEvent` and are
 * tracked by their trigger functions instead.
 */
import type * as maplibregl from 'maplibre-gl';
import { trackNav } from '../../../utils/telemetry';

/** Track user-initiated zoom (scroll wheel, pinch, double-click). Skip programmatic zooms. */
export function trackZoomEndNav(e: maplibregl.MapLibreEvent): void {
  const orig = e.originalEvent;
  if (!orig) return; // programmatic - already tracked by trigger functions
  if (orig instanceof WheelEvent) {
    trackNav('zoom', 'mouse');
  } else if (typeof TouchEvent !== 'undefined' && orig instanceof TouchEvent) {
    trackNav('zoom', 'touch');
  } else if (orig instanceof MouseEvent) {
    trackNav('zoom', 'dblclick');
  }
}

/** Track user-initiated pan (click-drag, touch-drag). Skip programmatic pans and zoom-induced moves. */
export function trackMoveEndNav(e: maplibregl.MapLibreEvent): void {
  const orig = e.originalEvent;
  if (!orig) return; // programmatic - already tracked by trigger functions
  if (orig instanceof WheelEvent) return; // zoom gesture - tracked by zoomend
  if (orig instanceof MouseEvent) {
    trackNav('pan', 'mouse');
  } else if (typeof TouchEvent !== 'undefined' && orig instanceof TouchEvent) {
    trackNav('pan', 'touch');
  }
}
