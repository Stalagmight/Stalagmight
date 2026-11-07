import type * as maplibregl from 'maplibre-gl';
import { useNetworkStore } from '../../store/networkStore';
import { registerTileProtocol } from '../../utils/tileProtocol';
import { hasLiveStyle } from './mapUtils';
import { createMap } from './createMap';

/**
 * The one place the app's three MapLibre maps are built (#576 D18): the survey
 * map (MapCanvas), the Global View (GlobalViewMap) and the section preview
 * (SectionPreview). Each repeated the same construction steps, and the copies had
 * drifted; this owns what they share:
 *
 * - the construction options every surface wants (no MapLibre attribution
 *   control - the app draws its own, see docs/map-renderer.md "Imagery
 *   attribution"), plus the per-surface ones passed in;
 * - interaction disabling: no rotation (mouse or two-finger twist) and no pitch -
 *   every surface is a top-down plan;
 * - the Android tile protocol, registered once and never unregistered
 *   (utils/tileProtocol.ts; a no-op on desktop, where Tauri serves the scheme);
 * - satellite reload on reconnect: when the app sees the connection come back
 *   (networkStore.tileReloadGeneration), the `satellite-tiles` source re-requests
 *   its tiles, including the ones that failed while offline.
 *
 * What stays with each surface: its style, camera, controls and layers.
 */

/** The raster source id every surface uses for its satellite imagery. */
export const SATELLITE_SOURCE_ID = 'satellite-tiles';

export interface SurveyMapOptions {
  container: HTMLElement;
  style: maplibregl.StyleSpecification;
  center: maplibregl.LngLatLike;
  zoom: number;
  minZoom?: number;
  maxZoom?: number;
  /**
   * MapLibre's own keyboard handler. Off where the app's shortcuts own the keys
   * (survey map, Global View: it double-zoomed with them); the dialog preview
   * keeps it.
   */
  keyboard: boolean;
  /**
   * MapLibre's box zoom. Off on the survey map, where Ctrl+drag pans instead
   * (docs/move-tools.md "Ctrl+Drag Panning").
   */
  boxZoom: boolean;
  /**
   * The survey map's render tuning: capped tile cache, no fades, no world
   * copies, and a cheap WebGL context (no antialias, no preserved buffer, a
   * software renderer allowed).
   */
  performanceTuned?: boolean;
}

/**
 * Build a map for one of the app's surfaces. See the module comment for what it owns.
 * Null when the machine cannot give MapLibre a WebGL2 context (createMap.ts); the
 * caller then shows `MapUnavailable` in place of the map.
 */
export function createSurveyMap(opts: SurveyMapOptions): maplibregl.Map | null {
  registerTileProtocol();

  const map = createMap({
    container: opts.container,
    style: opts.style,
    center: opts.center,
    zoom: opts.zoom,
    ...(opts.minZoom !== undefined ? { minZoom: opts.minZoom } : {}),
    ...(opts.maxZoom !== undefined ? { maxZoom: opts.maxZoom } : {}),
    // The app shows the imagery attribution itself; MapLibre cannot update a live
    // source's attribution, which the live fetch needs (docs/map-renderer.md).
    attributionControl: false,
    keyboard: opts.keyboard,
    boxZoom: opts.boxZoom,
    ...(opts.performanceTuned
      ? {
          // See: https://deepwiki.com/maplibre/maplibre-gl-js/5.2-performance-optimization-techniques
          maxTileCacheSize: 100, // Limit tile cache memory
          maxTileCacheZoomLevels: 5, // Limit zoom range caching
          fadeDuration: 0, // Disable fade animations for snappier response
          renderWorldCopies: false, // Not needed for cave maps (saves render work)
          // WebGL context optimizations (MapLibre v5+). Type assertion needed:
          // canvasContextAttributes is supported at runtime but not yet in the
          // MapLibre GL JS TypeScript definitions.
          ...({
            canvasContextAttributes: {
              antialias: false, // Disable AA for better performance
              preserveDrawingBuffer: false, // Allow buffer discard
              failIfMajorPerformanceCaveat: false, // Don't fail on software renderer
            },
          } as Record<string, unknown>),
        }
      : {}),
  });
  if (!map) return null;

  // Every surface is a top-down plan: no rotation (Ctrl+drag rotates by default;
  // the survey map pans on it instead), no pitch, and a pinch zooms only - a
  // two-finger twist would otherwise rotate.
  map.dragRotate.disable();
  map.touchPitch.disable();
  map.touchZoomRotate.disableRotation();

  const stopReload = reloadSatelliteOnReconnect(map);
  map.once('remove', stopReload);
  return map;
}

/**
 * Re-request every tile of the map's satellite source, failed ones included.
 *
 * `setTiles` with the source's own URLs is MapLibre's public way to do it: the
 * source reloads as changed data, and its tile manager then reloads every tile in
 * view as expired - an errored tile too, which a plain repaint never retries.
 * (The three copies this replaced called a `clearTiles` that lives on the tile
 * manager, not on the source `getSource` returns, so they never ran.)
 * Returns whether a reload was started.
 */
export function reloadSatelliteTiles(map: maplibregl.Map): boolean {
  // No style while the map is being removed or its WebGL context is lost.
  if (!hasLiveStyle(map)) return false;
  const source = map.getSource(SATELLITE_SOURCE_ID) as maplibregl.RasterTileSource | undefined;
  const tiles = source?.tiles;
  if (!source || typeof source.setTiles !== 'function' || !tiles || tiles.length === 0) return false;
  source.setTiles([...tiles]);
  return true;
}

/** Reload the satellite tiles whenever the app sees the connection return. Returns the unsubscribe. */
function reloadSatelliteOnReconnect(map: maplibregl.Map): () => void {
  return useNetworkStore.subscribe((state, prev) => {
    if (state.tileReloadGeneration !== prev.tileReloadGeneration) reloadSatelliteTiles(map);
  });
}
