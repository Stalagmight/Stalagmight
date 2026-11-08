import { useEffect, useRef } from 'react';
import * as maplibregl from 'maplibre-gl';
import { useTranslation } from 'react-i18next';

import { useGasPlanStore } from '../../store/gasPlanStore';
import { useUIStore } from '../../store/uiStore';
import type { GpsOrigin } from '../../types/config';
import type { Station, StationRef } from '../../types/project';
import { metersToMapCoords } from '../../utils/coordinates';

/**
 * The Plan workspace's *point* markers: the zero-datum stars, the ordered way points, and the
 * marked points (#298). All three are simple pins - place, click, remove - so they share one hook
 * and one lifecycle, unlike the turn-frontier dots and the draggable stage drops, which carry drag
 * and focus-restore machinery and stay in `MapCanvas`.
 *
 * Extracted from `MapCanvas.tsx` rather than added to it: that file is frozen by the
 * `pnpm lint:ts-file-size` ratchet (it sits well above the 1500-LOC warn line, so it may shrink but
 * not grow), and the two new marker kinds had to live somewhere.
 *
 * Every effect fully tears its markers down and rebuilds them, matching the sibling marker effects:
 * the counts here are tiny (a datum or two, a handful of route points), so rebuilding is cheaper
 * than diffing.
 */

interface GasPlanPointMarkersOptions {
  map: maplibregl.Map | null;
  mapLoaded: boolean;
  /** Plan workspace mode is active. Everything here is Plan-only. */
  showGasPlan: boolean;
  /** Stations of the active geometry, for looking up each ref's coordinates. */
  activeStations: Station[];
  gpsOrigin: GpsOrigin | null;
  waypoints: StationRef[];
  markedPoints: { station: StationRef; label: string | null }[];
  setOrigin: (ref: StationRef) => void;
}

/**
 * A `section:index` -> screen-position lookup over the active stations.
 *
 * Built ONCE per effect run rather than scanning per point: `activeStations` is the whole station
 * graph (tens of thousands on a real survey), and the marked-point effect re-runs on every
 * keystroke of a label, so a per-point `.find()` would be a full scan per character typed.
 */
function lngLatIndex(
  stations: Station[],
  gpsOrigin: GpsOrigin | null,
): (ref: StationRef) => [number, number] | null {
  const byKey = new Map<string, [number, number]>();
  for (const s of stations) {
    byKey.set(
      `${s.station_ref.section_id}:${s.station_ref.station_index}`,
      metersToMapCoords(s.coordinates.x, s.coordinates.y, gpsOrigin) as [number, number],
    );
  }
  return (ref) => byKey.get(`${ref.section_id}:${ref.station_index}`) ?? null;
}

/**
 * Wire a marker element as an activatable control: pointer click plus Enter/Space.
 *
 * `onKeyboardActivate` fires only for the key path. Activating a pin removes it, which rebuilds
 * every marker in the effect, so focus would otherwise fall to `<body>` and a keyboard user would
 * lose their place mid-list. The sibling turn markers solve this the same way (`pendingTurnFocusRef`
 * in MapCanvas); the extracted hook needs its own copy.
 */
function onActivate(el: HTMLElement, run: () => void, onKeyboardActivate?: () => void): void {
  el.addEventListener('click', (e) => {
    e.stopPropagation();
    run();
  });
  el.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onKeyboardActivate?.();
      run();
    }
  });
}

/**
 * After a keyboard removal rebuilt the list, put focus back where the user was: the pin that now
 * occupies the removed index, or the new last one when they removed the tail. `null` when nothing
 * is pending, so a pointer click never steals focus.
 */
function restoreFocus(
  pendingRef: { current: number | null },
  markers: maplibregl.Marker[],
): void {
  const pending = pendingRef.current;
  if (pending === null) return;
  pendingRef.current = null;
  if (markers.length === 0) return;
  const target = markers[Math.min(pending, markers.length - 1)];
  target?.getElement().focus();
}

/** Open the Plan right-click menu for `ref` at the pointer. */
function onContextMenu(el: HTMLElement, ref: StationRef): void {
  el.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();
    useUIStore.getState().openDialog('context-menu', {
      // Over the map canvas: no scroller can move this menu, and opening it
      // scrolls the panels via the selection it sets.
      survivesScroll: true,
      target: { type: 'station', ref },
      position: { x: e.clientX, y: e.clientY },
    });
  });
}

export function useGasPlanPointMarkers({
  map,
  mapLoaded,
  showGasPlan,
  activeStations,
  gpsOrigin,
  waypoints,
  markedPoints,
  setOrigin,
}: GasPlanPointMarkersOptions): void {
  const { t } = useTranslation();
  const datumMarkersRef = useRef<maplibregl.Marker[]>([]);
  const waypointMarkersRef = useRef<maplibregl.Marker[]>([]);
  const markedMarkersRef = useRef<maplibregl.Marker[]>([]);
  // Index a keyboard user just removed, so the rebuild can hand focus back (see `restoreFocus`).
  const pendingWaypointFocusRef = useRef<number | null>(null);
  const pendingMarkedFocusRef = useRef<number | null>(null);

  // Zero-datum star markers (#38): a black star (white outline) on each zero-datum station while
  // planning. The gas solve's "Entrance" default already starts from the zero datum(s); this marks
  // them so the user can SEE where they are and click one to pick it explicitly as the origin -
  // the one left-click that still sets the origin now that a plain map click sets the endpoint
  // (#298). Plan mode only; bounded by the datum count (~1).
  useEffect(() => {
    for (const m of datumMarkersRef.current) m.remove();
    datumMarkersRef.current = [];
    if (!map || !mapLoaded || !showGasPlan) return undefined;
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
      el.style.cssText =
        'cursor:pointer;line-height:0;filter:drop-shadow(0 0 1px rgba(0,0,0,0.6));';
      el.innerHTML =
        '<svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">' +
        '<path d="M12 17.27L18.18 21l-1.64-7.03L22 9.24l-7.19-.61L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21z" ' +
        'fill="#000000" stroke="#ffffff" stroke-width="1.5" stroke-linejoin="round"/></svg>';
      const ref = st.station_ref;
      onActivate(el, () => setOrigin(ref));
      datumMarkersRef.current.push(
        new maplibregl.Marker({ element: el }).setLngLat(lngLat).addTo(map),
      );
    }
    return () => {
      for (const m of datumMarkersRef.current) m.remove();
      datumMarkersRef.current = [];
    };
  }, [showGasPlan, activeStations, gpsOrigin, mapLoaded, map, setOrigin, t]);

  // Way-point markers (#298): a numbered teal pin at each ordered route point. The number IS the
  // visit order, which is the whole point of the feature - a bare pin would leave "which one do I
  // hit first?" invisible on the map. Left-click (or Enter/Space) removes it, so an accidental pick
  // is one click to undo; right-click opens the Plan menu for that station.
  useEffect(() => {
    for (const m of waypointMarkersRef.current) m.remove();
    waypointMarkersRef.current = [];
    // Drop any pending keyboard focus when the markers are not going to be rebuilt. Without this a
    // pin activated by keyboard right before leaving Plan leaves the index set, and the NEXT entry
    // into Plan would move focus onto a map pin the diver never touched.
    if (!map || !mapLoaded || !showGasPlan) {
      pendingWaypointFocusRef.current = null;
      return undefined;
    }
    const lngLatOf = lngLatIndex(activeStations, gpsOrigin);
    waypoints.forEach((ref, i) => {
      const lngLat = lngLatOf(ref);
      // A way point whose station left the active geometry simply has no pin; the engine reports it
      // as stalled and the panel row still lists (and can remove) it.
      if (!lngLat) return;
      const el = document.createElement('div');
      el.setAttribute('data-testid', 'gas-waypoint-marker');
      el.setAttribute('data-waypoint-index', String(i));
      el.setAttribute('role', 'button');
      el.setAttribute('tabindex', '0');
      el.setAttribute('aria-label', t('gasPanel.map.waypointAria', { n: i + 1 }));
      el.title = t('gasPanel.map.waypointTooltip', { n: i + 1 });
      el.style.cssText =
        'width:18px;height:18px;border-radius:50%;border:2px solid #ffffff;background:#16a085;' +
        'color:#ffffff;font:600 11px/14px sans-serif;text-align:center;' +
        'box-shadow:0 0 3px rgba(0,0,0,0.7);cursor:pointer;display:flex;' +
        'align-items:center;justify-content:center;';
      el.textContent = String(i + 1);
      onActivate(
        el,
        () => useGasPlanStore.getState().removeWaypoint(i),
        () => {
          pendingWaypointFocusRef.current = i;
        },
      );
      onContextMenu(el, ref);
      waypointMarkersRef.current.push(
        new maplibregl.Marker({ element: el }).setLngLat(lngLat).addTo(map),
      );
    });
    restoreFocus(pendingWaypointFocusRef, waypointMarkersRef.current);
    return () => {
      for (const m of waypointMarkersRef.current) m.remove();
      waypointMarkersRef.current = [];
    };
  }, [showGasPlan, waypoints, activeStations, gpsOrigin, mapLoaded, map, t]);

  // Marked-point markers (#298): a bookmark pin on each annotated station. Purely an annotation -
  // it never reaches the gas engine - so it is drawn in a deliberately non-route colour and cannot
  // be confused with a way point. The label (when set) is the tooltip; left-click removes it.
  useEffect(() => {
    for (const m of markedMarkersRef.current) m.remove();
    markedMarkersRef.current = [];
    if (!map || !mapLoaded || !showGasPlan) {
      pendingMarkedFocusRef.current = null;
      return undefined;
    }
    const lngLatOf = lngLatIndex(activeStations, gpsOrigin);
    markedPoints.forEach((mp, i) => {
      const lngLat = lngLatOf(mp.station);
      if (!lngLat) return;
      const el = document.createElement('div');
      el.setAttribute('data-testid', 'gas-marked-point-marker');
      el.setAttribute('data-marked-index', String(i));
      el.setAttribute('role', 'button');
      el.setAttribute('tabindex', '0');
      const label = mp.label ?? '';
      el.setAttribute(
        'aria-label',
        label
          ? t('gasPanel.map.markedLabelledAria', { label })
          : t('gasPanel.map.markedAria', { n: i + 1 }),
      );
      el.title = label || t('gasPanel.map.markedTooltip');
      el.style.cssText =
        'cursor:pointer;line-height:0;filter:drop-shadow(0 0 1px rgba(0,0,0,0.6));';
      el.innerHTML =
        '<svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">' +
        '<path d="M6 2h12v20l-6-5-6 5z" fill="#d81b60" stroke="#ffffff" stroke-width="1.5" ' +
        'stroke-linejoin="round"/></svg>';
      onActivate(
        el,
        () => useGasPlanStore.getState().removeMarkedPoint(i),
        () => {
          pendingMarkedFocusRef.current = i;
        },
      );
      onContextMenu(el, mp.station);
      markedMarkersRef.current.push(
        new maplibregl.Marker({ element: el }).setLngLat(lngLat).addTo(map),
      );
    });
    restoreFocus(pendingMarkedFocusRef, markedMarkersRef.current);
    return () => {
      for (const m of markedMarkersRef.current) m.remove();
      markedMarkersRef.current = [];
    };
  }, [showGasPlan, markedPoints, activeStations, gpsOrigin, mapLoaded, map, t]);
}
