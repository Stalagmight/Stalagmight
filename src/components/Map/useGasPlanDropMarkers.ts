import { useEffect, useRef } from 'react';
import * as maplibregl from 'maplibre-gl';
import { useTranslation } from 'react-i18next';

import { useGasPlanStore } from '../../store/gasPlanStore';
import { U_TURN_ARROW_PATH } from '../common/icons';
import type { GpsOrigin } from '../../types/config';
import type { GasReachField } from '../../types/ipc.generated';
import type { Station, StationRef } from '../../types/project';
import { metersToMapCoords } from '../../utils/coordinates';
import { stageDropReached } from '../../utils/gasReach';
import { markerHalo } from './gasMarkerShadow';

/** The stage-drop triangle ("drop the stage here"), in the marker's 24-unit viewBox. */
const DROP_PATH = 'M3 6h18L12 20z';

/**
 * The Plan workspace's staged-strategy ▼ drop markers (#233) plus the distinct final-turn marker
 * (#246): draggable/keyboard-nudgeable route pins, each carrying its own drag-snap, arrow-key
 * nudge, dbl-click reset and keyboard focus-restore machinery. Kept separate from the simpler
 * point-marker hook (zero-datum stars, way points, marked points) because that machinery is real
 * complexity, not shared with the plain place/click pins.
 *
 * Extracted from `MapCanvas.tsx` rather than added to it: that file is frozen by the
 * `pnpm lint:ts-file-size` ratchet (it sits well above the 1500-LOC warn line, so it may shrink but
 * not grow), and a new marker kind (#317) needed room.
 */

interface GasPlanDropMarkersOptions {
  map: maplibregl.Map | null;
  mapLoaded: boolean;
  /** Plan workspace mode is active. Everything here is Plan-only. */
  showGasPlan: boolean;
  gasReachField: GasReachField | null;
  /** Stations of the active geometry, for looking up each ref's coordinates. */
  activeStations: Station[];
  gpsOrigin: GpsOrigin | null;
  setStageDropOverride: (index: number, ref: StationRef | null) => void;
}

export function useGasPlanDropMarkers({
  map,
  mapLoaded,
  showGasPlan,
  gasReachField,
  activeStations,
  gpsOrigin,
  setStageDropOverride,
}: GasPlanDropMarkersOptions): void {
  const { t } = useTranslation();
  const gasDropMarkersRef = useRef<maplibregl.Marker[]>([]);
  const gasFinalTurnMarkerRef = useRef<maplibregl.Marker | null>(null);
  /// Where the scooters are parked (#317) - derived by the engine, never dragged.
  const gasScooterDropMarkerRef = useRef<maplibregl.Marker | null>(null);
  // Same story as the turn-frontier dots (`pendingTurnFocusRef` in MapCanvas): a keyboard-nudged /
  // reset ▼ drop marker (#246) rebuilds every drop marker on recompute, so remember the drop INDEX
  // (stable across the move - the ref itself shifts) to restore focus to its rebuilt element.
  const pendingDropFocusRef = useRef<number | null>(null);

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
    // Symmetric with the two teardowns above (and with the returned cleanup below, which already
    // clears all three): without this, an early return just below - mode switch, project switch,
    // or a field with no scooter drop - would leave a stale scooter marker's ref uncleared here
    // while the other two marker kinds are already gone, an asymmetry between what "start a fresh
    // build" clears for the three marker kinds this effect owns.
    gasScooterDropMarkerRef.current?.remove();
    gasScooterDropMarkerRef.current = null;
    if (!map || !mapLoaded || !showGasPlan || !gasReachField) return undefined;
    const mapInst = map;
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
      // A pinned drop the dive turns short of (#576 RV8c-2-D3) stands at its pin, past the
      // turnaround, greyed: the stage is carried the whole dive. The engine never flags one
      // over-reserve; the guard keeps the two states from ever drawing together.
      const reached = stageDropReached(gasReachField, i);
      const overReserve = reached && gasReachField.stage_drop_over_reserve[i] === true;
      // Read the override fresh from the store: this effect rebuilds on the recomputed FIELD (not on
      // the override write, see the deps note), so the store is the current source at rebuild time.
      const overridden = useGasPlanStore.getState().stageDropOverrides[i] != null;
      const el = document.createElement('div');
      el.setAttribute('data-testid', 'gas-drop-marker');
      el.setAttribute('data-drop-index', String(i));
      el.setAttribute('data-over-reserve', String(overReserve));
      el.setAttribute('data-reached', String(reached));
      el.setAttribute('data-overridden', String(overridden));
      // A slider along the route: draggable with the mouse, nudged one station with the arrow keys,
      // reset with Delete/Escape. valuemin/max/now expose the position on the route to AT.
      el.setAttribute('role', 'slider');
      el.setAttribute('tabindex', '0');
      el.setAttribute('aria-label', t('gasPanel.map.dropAria', { n: i + 1 }));
      // A drop the dive does not reach (#576 RV8c-2-D3) stands at its pin past the turnaround, one
      // step beyond the slider's last station: it reads the maximum with the not-reached text,
      // Left/Down pulls it back onto the turnaround, and Right/Up has nowhere deeper to go on the
      // route. (Its pin is not a route station, so looking it up on the route found nothing and the
      // slider read the origin, with Right moving the pin back to the route's first leg.)
      const curIndex = reached
        ? Math.max(
            0,
            routeRefs.findIndex(
              (r) => r.section_id === ref.section_id && r.station_index === ref.station_index
            )
          )
        : routeRefs.length;
      const lastIndex = Math.max(0, routeRefs.length - 1);
      el.setAttribute('aria-valuemin', '0');
      el.setAttribute('aria-valuemax', String(lastIndex));
      el.setAttribute('aria-valuenow', String(Math.min(curIndex, lastIndex)));
      if (!reached) {
        el.setAttribute('aria-valuetext', t('gasPanel.map.dropNotReachedTooltip', { n: i + 1 }));
      }
      el.title = !reached
        ? t('gasPanel.map.dropNotReachedTooltip', { n: i + 1 })
        : overReserve
          ? t('gasPanel.map.dropOverReserveTooltip', { n: i + 1 })
          : t('gasPanel.map.dropDraggableTooltip', { n: i + 1 });
      // The halo is drawn, not filtered (#605): see gasMarkerShadow.ts.
      el.style.cssText = 'line-height:0;cursor:grab;touch-action:none;';
      // Downward triangle = "drop the stage here"; red when the diver dragged it over-reserve (#246),
      // grey when the dive never gets there (#576 RV8c-2-D3).
      const fill = !reached ? '#8a8f98' : overReserve ? '#d64545' : '#e08a1e';
      el.innerHTML =
        '<svg width="20" height="20" viewBox="0 0 24 24" overflow="visible" aria-hidden="true">' +
        markerHalo(DROP_PATH, 20, 1.5) +
        `<path d="${DROP_PATH}" fill="${fill}" stroke="#ffffff" stroke-width="1.5" ` +
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
        setStageDropOverride(i, snapped);
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
        const from = pendingIdx >= 0 ? pendingIdx : curIndex;
        const j = from + delta;
        if (j < 0 || j > routeRefs.length - 1) return;
        const target = routeRefs[j];
        if (!target) return;
        pendingDropFocusRef.current = i;
        setStageDropOverride(i, target);
      };
      const resetDrop = () => {
        if (useGasPlanStore.getState().stageDropOverrides[i] == null) return false;
        pendingDropFocusRef.current = i;
        setStageDropOverride(i, null);
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
          'width:18px;height:18px;border-radius:3px;border:2px solid #ffffff;' +
          'background:#7b2fbf;box-shadow:0 0 4px rgba(0,0,0,0.7);pointer-events:none;' +
          'display:flex;align-items:center;justify-content:center;';
        el.innerHTML =
          '<svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true">' +
          `<path d="${U_TURN_ARROW_PATH}" fill="none" stroke="#ffffff" stroke-width="2.5" ` +
          'stroke-linecap="round" stroke-linejoin="round"/></svg>';
        gasFinalTurnMarkerRef.current = new maplibregl.Marker({ element: el })
          .setLngLat(lngLat)
          .addTo(mapInst);
      }
    }

    // Where the scooters end up (#317). Non-interactive, like the final-turn marker and for the
    // same reason: this point is DERIVED by the engine (the boundary after the last scootered
    // segment) rather than placed, so there is nothing here to drag. Move it by changing which
    // segments are flown on the scooter in the segments table; dragging this would be a second,
    // conflicting way to express the same state.
    const parkRef = gasReachField.scooter_drop_ref;
    if (parkRef) {
      const lngLat = lngLatFor(parkRef);
      if (lngLat) {
        const el = document.createElement('div');
        el.setAttribute('data-testid', 'gas-scooter-drop-marker');
        el.title = t('gasPanel.map.scooterDropTooltip');
        el.style.cssText =
          'width:18px;height:18px;border-radius:3px;border:2px solid #ffffff;' +
          'background:#1f7a8c;box-shadow:0 0 4px rgba(0,0,0,0.7);pointer-events:none;' +
          'display:flex;align-items:center;justify-content:center;';
        el.innerHTML =
          '<svg width="12" height="12" viewBox="0 0 24 24" aria-hidden="true">' +
          '<path d="M4 14h16M7 14a3 3 0 106 0M14 14a3 3 0 106 0M6 14V9h8l3 5" ' +
          'fill="none" stroke="#ffffff" stroke-width="2.5" stroke-linecap="round"/></svg>';
        gasScooterDropMarkerRef.current = new maplibregl.Marker({ element: el })
          .setLngLat(lngLat)
          .addTo(mapInst);
      }
    }

    return () => {
      for (const m of gasDropMarkersRef.current) m.remove();
      gasDropMarkersRef.current = [];
      gasFinalTurnMarkerRef.current?.remove();
      gasFinalTurnMarkerRef.current = null;
      gasScooterDropMarkerRef.current?.remove();
      gasScooterDropMarkerRef.current = null;
    };
    // NB: intentionally NOT keyed on `gasStageDropOverrides`: rebuild the ▼ markers only when the
    // recomputed FIELD lands (mirrors the turn markers). Keying on the override would rebuild against
    // the still-stale field first, consuming `pendingDropFocusRef` before the recompute and losing
    // keyboard focus; overrides are read fresh from the store where needed. The fetch effect in
    // MapCanvas IS keyed on the overrides, so a pin/reset always re-fetches and lands here via
    // `gasReachField`.
  }, [map, showGasPlan, gasReachField, setStageDropOverride, activeStations, gpsOrigin, mapLoaded, t]);
}
