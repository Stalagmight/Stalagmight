import { useEffect, useMemo, useRef, useCallback, useState, forwardRef, useImperativeHandle } from 'react';
import { useLatest } from '../../hooks/useLatest';
import { useTranslation } from 'react-i18next';
import * as maplibregl from 'maplibre-gl';
import type { ProjectMetadata } from '../../types/project';
import GlobalSurveyLayer, { type SystemGeometry, generateSystemColor } from './GlobalSurveyLayer';
import type { SurveyGeometry } from '../../types/survey';
import { useGlobalViewStore, type SystemGeometryState } from '../../store/globalViewStore';
import { ViewModeToggle, useViewMode, type ViewMode } from './ViewModeToggle';
import { getTileConfig } from '../../utils/tileConfig';
import { attachDragReleaseForwarder } from '../../utils/mapDragRelease';
import { getMapThemeColors, getThemeColor } from '../../utils/themeColors';
import { useUIStore } from '../../store/uiStore';
import { formatFixed, formatNumber } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { testIdPart } from '../../utils/testId';
import { MapAttribution } from '../Map/MapAttribution';
import { createSurveyMap, SATELLITE_SOURCE_ID } from '../Map/createSurveyMap';
import { useContextLossToast } from '../Map/useContextLossToast';
import { useParseIndicatorClearance } from '../../hooks/useParseIndicatorClearance';
import { useShallow } from 'zustand/react/shallow';
import { MapUnavailable } from '../Map/MapUnavailable';

/** Local storage key for persisting map position/zoom. */
const MAP_STATE_STORAGE_KEY = 'globalViewMapState';

/** Shape of persisted map state. */
interface PersistedMapState {
  center: [number, number]; // [lng, lat]
  zoom: number;
}

/**
 * Load persisted map state from local storage.
 * Returns null if no valid state is found.
 * Clamps restored zoom to the tile provider's supported range.
 */
function loadPersistedMapState(): PersistedMapState | null {
  try {
    const stored = localStorage.getItem(MAP_STATE_STORAGE_KEY);
    if (!stored) return null;

    const parsed = JSON.parse(stored);
    const tileConfig = getTileConfig();
    // Validate structure, types, and coordinate ranges
    // MapLibre throws on latitude outside [-90, 90]
    if (
      typeof parsed === 'object' &&
      parsed !== null &&
      Array.isArray(parsed.center) &&
      parsed.center.length === 2 &&
      typeof parsed.center[0] === 'number' &&
      typeof parsed.center[1] === 'number' &&
      typeof parsed.zoom === 'number' &&
      Number.isFinite(parsed.center[0]) &&
      Number.isFinite(parsed.center[1]) &&
      Number.isFinite(parsed.zoom) &&
      parsed.center[0] >= -180 &&
      parsed.center[0] <= 180 &&
      parsed.center[1] >= -90 &&
      parsed.center[1] <= 90
    ) {
      return {
        center: [parsed.center[0], parsed.center[1]],
        // Clamp zoom to provider's supported range
        zoom: Math.max(tileConfig.minZoom, Math.min(tileConfig.maxZoom, parsed.zoom)),
      };
    }
  } catch {
    // Invalid JSON or other error - ignore
  }
  return null;
}

/**
 * Save map state to local storage.
 */
function saveMapState(center: [number, number], zoom: number): void {
  try {
    const state: PersistedMapState = { center, zoom };
    localStorage.setItem(MAP_STATE_STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Local storage may be full or disabled - ignore
  }
}

/**
 * Handle exposed by GlobalViewMap for parent components.
 */
export interface GlobalViewMapHandle {
  /**
   * Get centroids of all GPS-positioned systems.
   * Returns a Map of file_path -> [lng, lat] centroid.
   * For systems with geometry, uses the average of all station positions.
   * For systems without geometry, uses gps_center.
   */
  getSystemCentroids: () => Map<string, [number, number]>;
}

interface GlobalViewMapProps {
  projects: ProjectMetadata[];
  selectedProjectId: string | null;
  onProjectSelect: (filePath: string) => void;
  onProjectOpen: (filePath: string) => void;
  /** Survey geometry data keyed by file_path. */
  geometryData?: Map<string, SurveyGeometry>;
  /** Loading states keyed by file_path. */
  geometryStates?: Map<string, SystemGeometryState>;
  /** Search filter term (owned by GlobalView, passed down for filtering) */
  searchTerm?: string;
  /** Called when user right-clicks on a system (for context menu). */
  onContextMenu?: (filePath: string, x: number, y: number) => void;
  /** Called to zoom to a specific system by file path. */
  zoomToSystem?: string | null;
  /** Called after zoom to system is handled, to clear the zoom request. */
  onZoomHandled?: () => void;
}

/** The spinning ring a marker shows while its system's geometry loads. */
function createMarkerSpinner(strokeLight: string, strokeColor: string): HTMLDivElement {
  const spinner = document.createElement('div');
  spinner.className = 'marker-spinner';
  spinner.style.cssText = `
    width: 10px;
    height: 10px;
    border: 2px solid ${strokeLight};
    border-top-color: ${strokeColor};
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  `;
  return spinner;
}

/** Stable empty defaults: a `new Map()` default parameter is a new object on every
 *  render, which every effect depending on these props would see as a change. Typed,
 *  so the props' values are checked where they are read (#576 NF-W3UI-4). */
const NO_GEOMETRY = new Map<string, SurveyGeometry>();
const NO_GEOMETRY_STATES = new Map<string, SystemGeometryState>();

/** A shot endpoint as `[lng, lat]`, or null unless it holds two finite numbers. The
 *  geometry is IPC data typed `Array<number>`, so the length is not guaranteed. */
function endpointLngLat(point: readonly number[]): [number, number] | null {
  const [lng, lat] = point;
  return lng !== undefined && lat !== undefined && Number.isFinite(lng) && Number.isFinite(lat) ? [lng, lat] : null;
}

interface GlobalViewMapInnerProps extends GlobalViewMapProps {
  /** Rebuild the map on a new WebGL context (the lost-context toast's action). */
  onReloadMap: () => void;
}

/**
 * Map component for GlobalView showing all GPS-positioned systems.
 * Uses MapLibre GL JS with satellite tiles.
 */
const GlobalViewMapInner = forwardRef<GlobalViewMapHandle, GlobalViewMapInnerProps>(function GlobalViewMap(
  {
    projects,
    selectedProjectId,
    onProjectSelect,
    onProjectOpen,
    geometryData = NO_GEOMETRY,
    geometryStates = NO_GEOMETRY_STATES,
    searchTerm = '',
    onContextMenu,
    zoomToSystem,
    onZoomHandled,
    onReloadMap,
  },
  ref
) {
  const mapContainer = useRef<HTMLDivElement>(null);
  const map = useRef<maplibregl.Map | null>(null);
  const markersRef = useRef<Map<string, maplibregl.Marker>>(new Map());
  const surveyLayerRef = useRef<GlobalSurveyLayer | null>(null);
  const errorPopupRef = useRef<maplibregl.Popup | null>(null);
  const markerTooltipRef = useRef<HTMLDivElement | null>(null);
  const contextMenuOpenRef = useRef(false);
  const [mapLoaded, setMapLoaded] = useState(false);
  const [webgl2Unavailable, setWebgl2Unavailable] = useState(false);
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  // Read by the marker tooltip at hover time: the markers are built once per
  // system set (#576 D10), so a precision captured at build time went stale when
  // the user changed it in Settings (#576 RV3-FE-NITS).
  const precisionRef = useLatest(precision);
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);
  // Buffer for geometry data that arrives before map is ready
  const pendingGeometryRef = useRef<Map<string, SurveyGeometry> | null>(null);
  // Store searchTerm in ref for access during map initialization (avoids stale closure)
  const searchTermRef = useLatest(searchTerm);
  // Flag to suppress persistence for "Fit All" button (temporary override per spec)
  const suppressNextPersistRef = useRef(false);

  // Access lazy loading functions and multi-select from store
  const {
    loadGeometryBatch,
    selectedSystemIds,
    selectSystem,
    toggleSystemSelection,
    cancelPendingLoads,
  } = useGlobalViewStore(
    useShallow((s) => ({
      loadGeometryBatch: s.loadGeometryBatch,
      selectedSystemIds: s.selectedSystemIds,
      selectSystem: s.selectSystem,
      toggleSystemSelection: s.toggleSystemSelection,
      cancelPendingLoads: s.cancelPendingLoads,
    })),
  );

  // View mode state (persisted in localStorage)
  const [viewMode, setViewMode] = useViewMode();
  // Store viewMode in ref for callbacks that need current value without closure staleness
  const viewModeRef = useLatest<ViewMode>(viewMode);
  // Read by the marker creation effect and the marker click handlers, so that
  // effect re-runs only when the SET of systems changes (#576 D10): a loading-state
  // or selection change restyles the existing markers (styles effect below)
  // instead of tearing down and rebuilding every marker's DOM.
  const geometryStatesRef = useLatest(geometryStates);
  const selectedProjectIdRef = useLatest(selectedProjectId);

  // The FPS-driven tolerance throttle was removed with the GeoJSON source (#mem):
  // GlobalSurveyLayer now serves bulk geometry as on-demand vector tiles, which
  // bound rendered geometry to the visible tiles. GeoJSON `tolerance` (whole-
  // source simplification) has no vector-tile equivalent, so there is nothing
  // left for an FPS monitor to throttle.

  // Filter to only projects with GPS coordinates (null check only, no sentinel).
  // Memoized (#576 D10): four effects depend on it, and a fresh array on every
  // render re-ran the lazy-load effect on every store write - which, with a
  // failing system, retried it in an endless loop.
  const gpsProjects = useMemo(() => projects.filter((p) => p.gps_center !== null), [projects]);

  // The latest systems and geometry, for the imperative handle and the map's
  // `load` handler (which is registered once, on mount).
  const gpsProjectsRef = useLatest(gpsProjects);
  const geometryDataRef = useLatest(geometryData);

  // Store callbacks in refs to avoid stale closures in map event handlers
  // (declared before the init effect, whose listeners call them)
  const onProjectSelectRef = useLatest(onProjectSelect);
  const onProjectOpenRef = useLatest(onProjectOpen);
  const selectSystemRef = useLatest(selectSystem);
  const toggleSystemSelectionRef = useLatest(toggleSystemSelection);
  const onContextMenuRef = useLatest(onContextMenu);

  // Expose imperative handle for parent to get system centroids
  useImperativeHandle(ref, () => ({
    getSystemCentroids: () => {
      const centroids = new Map<string, [number, number]>();

      for (const project of gpsProjectsRef.current) {
        const geometry = geometryDataRef.current.get(project.file_path);

        if (geometry && geometry.shots.length > 0) {
          // Calculate centroid from station positions
          const stations = new Set<string>();
          let sumLng = 0;
          let sumLat = 0;

          for (const shot of geometry.shots) {
            // An endpoint without two finite numbers used to be summed anyway and
            // turned the whole centroid into NaN, so the arrow keys could never
            // reach the system (#576 NF-W3UI-4). Skip it instead.
            for (const point of [shot.from, shot.to]) {
              const lngLat = endpointLngLat(point);
              if (!lngLat) continue;
              const key = `${lngLat[0]},${lngLat[1]}`;
              if (stations.has(key)) continue;
              stations.add(key);
              sumLng += lngLat[0];
              sumLat += lngLat[1];
            }
          }

          if (stations.size > 0) {
            centroids.set(project.file_path, [sumLng / stations.size, sumLat / stations.size]);
          } else if (project.gps_center) {
            // No usable station position: the same fallback as no geometry.
            centroids.set(project.file_path, project.gps_center);
          }
        } else if (project.gps_center) {
          // Fall back to gps_center if no geometry loaded
          centroids.set(project.file_path, project.gps_center);
        }
      }

      return centroids;
    },
  }), [geometryDataRef, gpsProjectsRef]);

  // Calculate raw bounds (no padding) for all systems
  // Note: These are the actual bounds of all systems, without any span limiting
  const calculateRawBounds = useCallback((): maplibregl.LngLatBounds | null => {
    if (gpsProjects.length === 0) {
      return null;
    }

    // Use individual project bounds if available, otherwise use center points
    let minLng = Infinity;
    let maxLng = -Infinity;
    let minLat = Infinity;
    let maxLat = -Infinity;

    for (const project of gpsProjects) {
      if (project.gps_bounds) {
        minLng = Math.min(minLng, project.gps_bounds.west);
        maxLng = Math.max(maxLng, project.gps_bounds.east);
        minLat = Math.min(minLat, project.gps_bounds.south);
        maxLat = Math.max(maxLat, project.gps_bounds.north);
      } else if (project.gps_center) {
        const [lng, lat] = project.gps_center;
        minLng = Math.min(minLng, lng);
        maxLng = Math.max(maxLng, lng);
        minLat = Math.min(minLat, lat);
        maxLat = Math.max(maxLat, lat);
      }
    }

    return new maplibregl.LngLatBounds([minLng, minLat], [maxLng, maxLat]);
  }, [gpsProjects]);

  // Calculate bounds with padding factor
  // MIN_SPAN_DEG ensures single-point or line cases get reasonable bounds (~1km)
  const MIN_SPAN_DEG = 0.01;
  const addPaddingToBounds = useCallback((bounds: maplibregl.LngLatBounds, paddingFactor: number): maplibregl.LngLatBounds => {
    const sw = bounds.getSouthWest();
    const ne = bounds.getNorthEast();
    // Enforce minimum span to avoid degenerate bounds for single-point/line cases
    const lngSpan = Math.max(ne.lng - sw.lng, MIN_SPAN_DEG);
    const latSpan = Math.max(ne.lat - sw.lat, MIN_SPAN_DEG);

    // Add padding as percentage of span
    const lngPadding = lngSpan * paddingFactor;
    const latPadding = latSpan * paddingFactor;

    // For single-point case, center the bounds on the point
    const centerLng = (sw.lng + ne.lng) / 2;
    const centerLat = (sw.lat + ne.lat) / 2;
    const halfLngSpan = lngSpan / 2 + lngPadding;
    const halfLatSpan = latSpan / 2 + latPadding;

    return new maplibregl.LngLatBounds(
      [centerLng - halfLngSpan, Math.max(-85, centerLat - halfLatSpan)],
      [centerLng + halfLngSpan, Math.min(85, centerLat + halfLatSpan)]
    );
  }, []);

  // Calculate bounds to fit all systems (for Fit All button and initial view)
  const calculateBounds = useCallback(() => {
    const rawBounds = calculateRawBounds();
    if (!rawBounds) {
      // Default to world view if no GPS projects
      return new maplibregl.LngLatBounds([-180, -85], [180, 85]);
    }
    return rawBounds;
  }, [calculateRawBounds]);
  const calculateBoundsRef = useLatest(calculateBounds);

  // Soft bounds (20% padding) - animate back if user pans beyond this
  const softBoundsRef = useRef<maplibregl.LngLatBounds | null>(null);

  // Calculate hard bounds (50% padding) for maxBounds constraint
  const calculateHardBounds = useCallback((): maplibregl.LngLatBounds | null => {
    const rawBounds = calculateRawBounds();
    if (!rawBounds) return null;
    return addPaddingToBounds(rawBounds, 0.5);
  }, [calculateRawBounds, addPaddingToBounds]);

  // Update soft and hard bounds when projects change
  // This ensures newly added/removed systems are reflected in pan constraints
  useEffect(() => {
    const rawBounds = calculateRawBounds();
    if (rawBounds) {
      softBoundsRef.current = addPaddingToBounds(rawBounds, 0.2);
    } else {
      softBoundsRef.current = null;
    }

    // Update map's minZoom if map is initialized
    // No maxBounds constraint - allow free panning
    if (map.current) {
      // Remove any existing maxBounds constraint
      map.current.setMaxBounds(undefined);
      // No minZoom limit - allow unlimited zoom out
      map.current.setMinZoom(0);
    }
  }, [calculateRawBounds, addPaddingToBounds, calculateHardBounds]);

  // Initialize map
  useEffect(() => {
    if (!mapContainer.current || map.current) return;

    // Through the ref like the load handler below: this effect runs once, on
    // mount (its cleanup tears the map down), so it must not depend on
    // calculateBounds. At mount the ref already holds the current one.
    const bounds = calculateBoundsRef.current();
    const defaultCenter = bounds.getCenter();

    // Try to restore persisted map state
    const persistedState = loadPersistedMapState();
    // Use persisted state if available, otherwise use calculated bounds center
    const initialCenter: [number, number] = persistedState
      ? persistedState.center
      : [defaultCenter.lng, defaultCenter.lat];
    const initialZoom = persistedState?.zoom ?? 10;

    // No maxBounds - allow free panning and zooming
    const tileConfig = getTileConfig();
    map.current = createSurveyMap({
      container: mapContainer.current,
      style: {
        version: 8,
        sources: {
          [SATELLITE_SOURCE_ID]: {
            type: 'raster',
            tiles: tileConfig.tiles,
            tileSize: tileConfig.tileSize,
            attribution: tileConfig.attribution,
            minzoom: tileConfig.minZoom,
            maxzoom: tileConfig.maxZoom,
          },
        },
        layers: [
          {
            id: 'satellite-tiles',
            type: 'raster',
            source: SATELLITE_SOURCE_ID,
            paint: {
              'raster-opacity': 1,
            },
          },
        ],
      },
      center: initialCenter,
      zoom: initialZoom,
      minZoom: 0, // Dynamic minZoom set after load based on system bounds
      maxZoom: tileConfig.maxZoom,
      // MapLibre's keyboard handler would double-zoom with the app's shortcuts.
      keyboard: false,
      boxZoom: true,
    });
    // No WebGL2 context (createMap.ts): show MapUnavailable in place of the map.
    if (!map.current) {
      setWebgl2Unavailable(true);
      return;
    }

    // Add navigation controls
    map.current.addControl(
      new maplibregl.NavigationControl({ showCompass: false }),
      'top-right'
    );

    // Add scale control
    map.current.addControl(
      new maplibregl.ScaleControl({ maxWidth: 100, unit: 'metric' }),
      'bottom-left'
    );

    map.current.on('load', () => {
      // The style loads asynchronously, so the systems may have changed since
      // this effect ran on mount: read them (and their bounds) as they are NOW.
      // The mount-time list used to drop a system that arrived in between from
      // the first fit and from the buffered geometry (#576 D17).
      const projectsNow = gpsProjectsRef.current;
      // Fit to bounds after load only if no persisted state
      // (persisted state is already applied via initial center/zoom)
      if (projectsNow.length > 0 && map.current && !persistedState) {
        map.current.fitBounds(calculateBoundsRef.current(), { padding: 50, maxZoom: 15 });
      }

      // Initialize survey layer now that map is loaded
      if (!surveyLayerRef.current && map.current) {
        // Disable default double-click zoom so our handler can open projects
        map.current.doubleClickZoom.disable();

        surveyLayerRef.current = new GlobalSurveyLayer(map.current, {
          onSystemClick: (filePath, _event, ctrlKey) => {
            if (ctrlKey) {
              // Ctrl+click: toggle selection (multi-select)
              // Don't call onProjectSelect to avoid resetting to single-select
              toggleSystemSelectionRef.current(filePath);
            } else {
              // Regular click: select single system in store and notify parent
              selectSystemRef.current(filePath);
              onProjectSelectRef.current(filePath);
            }
          },
          onSystemDoubleClick: (filePath) => {
            onProjectOpenRef.current(filePath);
          },
          onSystemHover: () => {
            // Hover handling could update metadata panel preview
          },
          onSystemContextMenu: (filePath, event) => {
            event.preventDefault();
            onContextMenuRef.current?.(filePath, event.clientX, event.clientY);
          },
        });

        // Apply any pending geometry that arrived before map was ready
        // Only apply if still in survey mode (user may have toggled to markers while map was loading)
        if (pendingGeometryRef.current && pendingGeometryRef.current.size > 0 && viewModeRef.current === 'survey') {
          const systems: SystemGeometry[] = [];
          for (const project of projectsNow) {
            const geometry = pendingGeometryRef.current.get(project.file_path);
            if (geometry) {
              systems.push({ metadata: project, geometry });
            }
          }
          if (systems.length > 0) {
            surveyLayerRef.current.update(systems);
          }
        }
        // Always clear pending ref after processing (whether applied or not)
        pendingGeometryRef.current = null;

        // Apply current search filter immediately after layer construction
        surveyLayerRef.current.setSearchFilter(searchTermRef.current || null);
      }

      // Trigger re-render so effects that depend on mapLoaded can run
      setMapLoaded(true);
    });

    // Click on map (not on marker/system) clears selection
    const handleMapClick = () => {
      // Clear selection - markers use stopPropagation so this only fires for empty space
      // Survey layer has its own click handling that also clears on empty space
      selectSystemRef.current(null);
      onProjectSelectRef.current('');
    };
    map.current.on('click', handleMapClick);

    // Rubber-band effect: when user releases pan outside soft bounds, animate back
    // This creates a natural "spring" feel without requiring internal MapLibre APIs
    let isRubberBanding = false;
    let rubberBandIdleHandler: (() => void) | null = null;

    const handleMoveEndRubberBand = (e: maplibregl.MapMovementEvent) => {
      if (!map.current || !softBoundsRef.current) return;

      // Skip if we're already in a rubber-band animation (prevents infinite loop)
      if (isRubberBanding) return;

      // Only apply rubber-band effect for user-initiated moves (not programmatic moves)
      // originalEvent is present when the move was user-initiated
      if (!e.originalEvent) return;

      const mapCenter = map.current.getCenter();
      const softBounds = softBoundsRef.current;

      // Check if center is outside soft bounds
      let targetLng = mapCenter.lng;
      let targetLat = mapCenter.lat;
      let needsCorrection = false;

      const sw = softBounds.getSouthWest();
      const ne = softBounds.getNorthEast();

      if (mapCenter.lng < sw.lng) {
        targetLng = sw.lng;
        needsCorrection = true;
      } else if (mapCenter.lng > ne.lng) {
        targetLng = ne.lng;
        needsCorrection = true;
      }

      if (mapCenter.lat < sw.lat) {
        targetLat = sw.lat;
        needsCorrection = true;
      } else if (mapCenter.lat > ne.lat) {
        targetLat = ne.lat;
        needsCorrection = true;
      }

      if (needsCorrection) {
        // Set flag to prevent re-triggering during animation
        isRubberBanding = true;

        // Animate back to soft bounds with gentle easing
        map.current.easeTo({
          center: [targetLng, targetLat],
          duration: 300,
          easing: (t) => 1 - Math.pow(1 - t, 3), // Ease out cubic
        });

        // Clear flag when animation completes using one-shot idle event
        // This is more reliable than setTimeout as it handles interrupted animations
        rubberBandIdleHandler = () => {
          isRubberBanding = false;
          rubberBandIdleHandler = null;
        };
        map.current.once('idle', rubberBandIdleHandler);
      }
    };

    map.current.on('moveend', handleMoveEndRubberBand);

    // Persist map position/zoom to local storage on every moveend
    // Exception: "Fit All" button sets suppressNextPersistRef to skip one persist
    // This ensures state persists across sessions while Fit All overrides temporarily per spec
    const handleMoveEndPersist = () => {
      if (!map.current) return;
      // Check and clear suppress flag (used by Fit All button)
      if (suppressNextPersistRef.current) {
        suppressNextPersistRef.current = false;
        return;
      }
      const center = map.current.getCenter();
      const zoom = map.current.getZoom();
      saveMapState([center.lng, center.lat], zoom);
    };
    map.current.on('moveend', handleMoveEndPersist);

    // A drag released over floating chrome (the navigation/scale controls, the
    // surrounding panels) would otherwise leave the map stuck "moving" - so the
    // moveend above, which is what persists the viewport, would never fire;
    // src/utils/mapDragRelease.ts has why MapLibre needs the help.
    const detachDragReleaseForwarder = attachDragReleaseForwarder(() => map.current);

    return () => {
      detachDragReleaseForwarder();
      // Clean up rubber-band handler and any pending idle listener
      if (rubberBandIdleHandler && map.current) {
        map.current.off('idle', rubberBandIdleHandler);
      }
      map.current?.off('moveend', handleMoveEndRubberBand);
      map.current?.off('moveend', handleMoveEndPersist);
      map.current?.off('click', handleMapClick);
      // Clean up survey layer
      surveyLayerRef.current?.destroy();
      surveyLayerRef.current = null;
      // Clean up error popup
      errorPopupRef.current?.remove();
      errorPopupRef.current = null;
      // Clean up marker tooltip
      markerTooltipRef.current?.remove();
      markerTooltipRef.current = null;
      // Clean up markers
      markersRef.current.forEach((marker) => marker.remove());
      markersRef.current.clear();
      map.current?.remove();
      map.current = null;
    };
  }, [calculateBoundsRef, gpsProjectsRef, onContextMenuRef, onProjectOpenRef, onProjectSelectRef, searchTermRef, selectSystemRef, toggleSystemSelectionRef, viewModeRef]);

  // A WebGL context that is lost and does not come back within a few seconds
  // leaves every system's survey blank for good: say so, and offer the rebuild
  // (#576 D38-LOST).
  useContextLossToast(map, mapLoaded, onReloadMap);
  // [#576 W71-NF10] The import parse indicator floats above the imagery
  // attribution, as in the project view: centred on the window, it reaches over
  // the source names in this map's bottom-right corner.
  const attributionParseIndicatorClearance = useParseIndicatorClearance('global-view-map-attribution');
  // [#576 W79-NF8] And above the systems count, which sits just above the
  // attribution in that corner, so is the higher edge: in a narrow window
  // (800 or 900 px) the pill's right end reaches over it.
  const systemCountParseIndicatorClearance = useParseIndicatorClearance('global-view-map-system-count');

  // E2E handle on the Global View map (the survey map's is `__stalagmight_map__`).
  useEffect(() => {
    if (!(import.meta.env.DEV || import.meta.env.MODE === 'test' || import.meta.env.VITE_E2E === '1')) return undefined;
    const w = window as unknown as { __stalagmight_global_map__?: maplibregl.Map };
    if (map.current) w.__stalagmight_global_map__ = map.current;
    return () => {
      delete w.__stalagmight_global_map__;
    };
  }, [mapLoaded]);

  // Update survey layer when geometry data changes (from props)
  useEffect(() => {
    if (geometryData.size === 0) return;

    // Build system geometry array
    const systems: SystemGeometry[] = [];
    for (const project of gpsProjects) {
      const geometry = geometryData.get(project.file_path);
      if (geometry) {
        systems.push({ metadata: project, geometry });
      }
    }

    if (surveyLayerRef.current) {
      // Layer ready - apply immediately
      surveyLayerRef.current.update(systems);
      pendingGeometryRef.current = null;
    } else {
      // Layer not ready yet - buffer for when it initializes
      pendingGeometryRef.current = geometryData;
    }
  }, [geometryData, gpsProjects]);

  // Update survey layer selection when selected systems change (from store)
  useEffect(() => {
    surveyLayerRef.current?.setSelectedSystems(selectedSystemIds);
  }, [selectedSystemIds]);

  // Apply theme colors to survey layer when theme changes
  useEffect(() => {
    if (surveyLayerRef.current) {
      surveyLayerRef.current.applyThemeColors(getMapThemeColors());
    }
  }, [resolvedTheme, mapLoaded]);

  // Sync precision to survey layer
  useEffect(() => {
    if (surveyLayerRef.current) {
      surveyLayerRef.current.setPrecision(precision);
    }
  }, [precision]);

  // Sync search filter to survey layer (searchTerm is owned by GlobalView, passed as prop)
  useEffect(() => {
    surveyLayerRef.current?.setSearchFilter(searchTerm || null);
  }, [searchTerm]);

  // Sync single-selection prop to store for compatibility
  useEffect(() => {
    // If selectedProjectId from props changes, update the store
    // This maintains backward compatibility with existing code
    // Handle both truthy and null/undefined cases
    selectSystem(selectedProjectId);
  }, [selectedProjectId, selectSystem]);

  // Handle view mode changes - cancel pending loads and update visibility
  const handleViewModeChange = useCallback((newMode: ViewMode) => {
    setViewMode(newMode);

    // Cancel any in-progress geometry loads when switching to markers mode
    // (preserves already-loaded geometry so it's available when switching back)
    if (newMode === 'markers') {
      cancelPendingLoads();
      // Clear pending geometry buffer to prevent stale data from being applied
      pendingGeometryRef.current = null;
    }

    // Update survey layer visibility
    surveyLayerRef.current?.setVisible(newMode === 'survey');
  }, [setViewMode, cancelPendingLoads]);

  // Sync view mode to survey layer when it becomes ready
  useEffect(() => {
    if (surveyLayerRef.current) {
      surveyLayerRef.current.setVisible(viewMode === 'survey');
    }
  }, [mapLoaded, viewMode]);

  // Lazy load geometry for visible systems when map loads or moveend
  // Only load when in survey mode
  //
  // #576 D10-D1, retry on user action only: a system whose load failed stays an
  // error marker. This effect re-runs on exactly the user's retry actions -
  // switching (back) to the survey view, or the recent list reloading (a new
  // `projects` array) - so only its first pass retries failed systems; a map
  // move loads newly visible ones and leaves failures alone. Clicking an error
  // marker retries that one system (marker effect below).
  useEffect(() => {
    if (!map.current || !mapLoaded) return;
    // Don't load geometry when in markers-only mode
    if (viewMode !== 'survey') return;

    const loadVisibleSystems = (retryErrors: boolean) => {
      if (!map.current) return;
      // Check ref for current mode (avoid stale closure)
      if (viewModeRef.current !== 'survey') return;

      const bounds = map.current.getBounds();
      const center = bounds.getCenter();

      // Find systems in view and sort by distance from center
      const visibleSystems = gpsProjects
        .filter((p) => {
          if (!p.gps_center) return false;
          const [lng, lat] = p.gps_center;
          return bounds.contains([lng, lat]);
        })
        .map((p) => {
          const [lng, lat] = p.gps_center!;
          const dist = Math.sqrt(
            Math.pow(lng - center.lng, 2) + Math.pow(lat - center.lat, 2)
          );
          return { project: p, dist };
        })
        .sort((a, b) => a.dist - b.dist)
        .map((item) => item.project.file_path);

      // Load systems sequentially in priority order (loadGeometryBatch handles this)
      // Don't await - let it run in background so map stays responsive
      void loadGeometryBatch(visibleSystems, { retryErrors });
    };

    // Load visible systems now
    loadVisibleSystems(true);

    // Load visible systems on map movement
    const handleMoveEnd = () => loadVisibleSystems(false);
    map.current.on('moveend', handleMoveEnd);

    return () => {
      map.current?.off('moveend', handleMoveEnd);
    };
  }, [mapLoaded, gpsProjects, loadGeometryBatch, viewMode, viewModeRef]);

  // Show error popup for a failed geometry load
  const showErrorPopup = useCallback((filePath: string, name: string, error: string, lng: number, lat: number) => {
    if (!map.current) return;

    // Remove existing error popup if any
    if (errorPopupRef.current) {
      errorPopupRef.current.remove();
    }

    // Build popup content using DOM to prevent XSS (per memory: use textContent, not innerHTML)
    const container = document.createElement('div');
    container.style.cssText = `color: ${getThemeColor('--cc-text-primary')}; padding: 8px; max-width: 300px;`;

    const title = document.createElement('div');
    title.style.cssText = `font-weight: 600; margin-bottom: 4px; color: ${getThemeColor('--cc-error')};`;
    title.textContent = t('globalView.map.errorLoading', { name });
    container.appendChild(title);

    const errorDiv = document.createElement('div');
    errorDiv.style.cssText = `font-size: 0.85em; color: ${getThemeColor('--cc-gray-line')}; word-wrap: break-word;`;
    errorDiv.textContent = error;
    container.appendChild(errorDiv);

    const pathDiv = document.createElement('div');
    pathDiv.style.cssText = `font-size: 0.75em; color: ${getThemeColor('--cc-gray-text')}; margin-top: 8px; word-wrap: break-word;`;
    pathDiv.textContent = filePath;
    container.appendChild(pathDiv);

    errorPopupRef.current = new maplibregl.Popup({
      closeButton: true,
      closeOnClick: true,
      maxWidth: '320px',
      offset: 15,
    })
      .setLngLat([lng, lat])
      .setDOMContent(container)
      .addTo(map.current);
  }, [t]);

  // Create the markers when the set of systems (or the theme) changes. Loading
  // state and selection are read from refs and kept current by the styles and
  // visibility effects below, so the markers are not rebuilt on every system load
  // (#576 D10: N systems loading rebuilt all N markers about 2N times).
  useEffect(() => {
    if (!map.current || !mapLoaded) return;

    const strokeColor = getThemeColor('--cc-stroke-color');
    const strokeLight = getThemeColor('--cc-stroke-light');
    const shadow = getThemeColor('--cc-map-ctrl-shadow');
    const tooltipBg = getThemeColor('--cc-preview-bg');
    const tooltipText = getThemeColor('--cc-depth-text');

    // Remove old markers
    markersRef.current.forEach((marker) => marker.remove());
    markersRef.current.clear();

    // Create tooltip element if it doesn't exist (shared across all markers)
    if (!markerTooltipRef.current) {
      const tooltip = document.createElement('div');
      tooltip.className = 'globalview-marker-tooltip';
      map.current.getContainer().appendChild(tooltip);
      markerTooltipRef.current = tooltip;
    }
    // Always re-apply tooltip styles so theme changes are reflected
    markerTooltipRef.current.style.cssText = `
      position: absolute;
      pointer-events: none;
      background-color: ${tooltipBg};
      color: ${tooltipText};
      padding: 8px 12px;
      border-radius: 4px;
      font-size: 12px;
      line-height: 1.4;
      z-index: 1000;
      white-space: nowrap;
      box-shadow: 0 2px 8px ${shadow};
      display: none;
    `;

    // Add markers for each GPS project
    for (const project of gpsProjects) {
      if (!project.gps_center) continue;

      const [lng, lat] = project.gps_center;
      const loadingState = geometryStatesRef.current.get(project.file_path)?.state ?? 'idle';
      const isSelected = selectedProjectIdRef.current === project.file_path;

      // Use same color as survey mode (deterministic based on file path)
      // Error state overrides with red
      const systemColor = loadingState === 'error'
        ? getThemeColor('--cc-error')
        : generateSystemColor(project.file_path);

      // Create container element (fixed size, handles positioning)
      const el = document.createElement('div');
      el.className = 'globalview-system-marker';
      el.setAttribute('data-testid', `system-marker-${testIdPart(project.file_path)}`);
      el.style.cssText = `
        width: 24px;
        height: 24px;
        cursor: pointer;
        display: flex;
        align-items: center;
        justify-content: center;
      `;

      // Create inner visual marker (can scale from center)
      const inner = document.createElement('div');
      inner.className = 'marker-inner';
      inner.style.cssText = `
        width: 20px;
        height: 20px;
        border-radius: 50%;
        background-color: ${systemColor};
        border: 2px solid ${isSelected ? strokeColor : strokeLight};
        box-shadow: ${isSelected ? `0 0 0 3px ${strokeLight}, ` : ''}0 2px 4px ${shadow};
        transition: transform 0.15s ease-out, border-color 0.2s, box-shadow 0.2s;
        display: flex;
        align-items: center;
        justify-content: center;
      `;
      el.appendChild(inner);

      // Add spinner for loading state
      if (loadingState === 'loading') inner.appendChild(createMarkerSpinner(strokeLight, strokeColor));

      // Hover effect - scale inner element (grows uniformly from center)
      el.addEventListener('mouseenter', () => {
        inner.style.transform = 'scale(1.3)';

        // Don't show tooltip if context menu is open
        if (contextMenuOpenRef.current) return;

        // Build tooltip content (same format as survey view)
        if (markerTooltipRef.current) {
          const tooltip = markerTooltipRef.current;
          const projectName = el.dataset.projectName || t('globalView.tooltip.unknown');
          const totalLength = parseFloat(el.dataset.totalLength || '0');
          const stationCount = parseInt(el.dataset.stationCount || '0', 10);

          // Format length like survey view
          let lengthStr: string;
          const lengthDigits = precisionRef.current.length;
          if (totalLength >= 1000) {
            lengthStr = `${formatFixed(totalLength / 1000, undefined, lengthDigits)} ${t('common.units.kilometers')}`;
          } else {
            lengthStr = `${formatFixed(totalLength, undefined, lengthDigits)} ${t('common.units.meters')}`;
          }

          // Build tooltip content using DOM to prevent XSS (matches survey view format)
          tooltip.textContent = '';

          // Name row (bold)
          const nameDiv = document.createElement('div');
          nameDiv.style.cssText = 'font-weight: 600; margin-bottom: 4px;';
          nameDiv.textContent = projectName;
          tooltip.appendChild(nameDiv);

          // Length row
          const lengthDiv = document.createElement('div');
          lengthDiv.style.color = getThemeColor('--cc-gray-text');
          lengthDiv.textContent = t('globalView.tooltip.length', { length: lengthStr });
          tooltip.appendChild(lengthDiv);

          // Station count row
          const stationDiv = document.createElement('div');
          stationDiv.style.color = getThemeColor('--cc-gray-text');
          stationDiv.textContent = t('globalView.tooltip.stations', {
            count: stationCount,
            countFormatted: formatNumber(stationCount),
          });
          tooltip.appendChild(stationDiv);

          tooltip.style.transform = '';
          tooltip.style.display = 'block';
        }
      });

      // Follow mouse while hovering (position at bottom-right of cursor like survey view)
      el.addEventListener('mousemove', (e) => {
        // Don't update tooltip if context menu is open
        if (contextMenuOpenRef.current) return;

        if (markerTooltipRef.current && map.current) {
          const tooltip = markerTooltipRef.current;
          const container = map.current.getContainer();
          const rect = container.getBoundingClientRect();

          // Position 15px offset from cursor (bottom-right)
          const x = e.clientX - rect.left + 15;
          const y = e.clientY - rect.top + 15;

          // Measure tooltip and clamp to container bounds
          const tooltipRect = tooltip.getBoundingClientRect();
          const maxX = container.clientWidth - tooltipRect.width - 10;
          const maxY = container.clientHeight - tooltipRect.height - 10;

          tooltip.style.left = `${Math.max(0, Math.min(x, maxX))}px`;
          tooltip.style.top = `${Math.max(0, Math.min(y, maxY))}px`;
        }
      });

      el.addEventListener('mouseleave', () => {
        inner.style.transform = 'scale(1)';

        // Clear context menu flag (menu closes when mouse leaves)
        contextMenuOpenRef.current = false;

        // Hide tooltip
        if (markerTooltipRef.current) {
          markerTooltipRef.current.style.display = 'none';
        }
      });

      // Click handler - for error markers, show error popup instead of selecting
      el.addEventListener('click', (e) => {
        e.stopPropagation();
        // The state NOW, not at creation: the marker outlives its loads.
        const errorState = geometryStatesRef.current.get(project.file_path);
        if (errorState?.state === 'error') {
          // Show error popup on click of red error marker
          const errorMsg = errorState.error ?? 'Unknown error';
          showErrorPopup(project.file_path, project.name, errorMsg, lng, lat);
          // The user's retry (#576 D10-D1): a file that was locked or still being
          // copied may load now. Survey view only - markers mode loads no geometry.
          if (viewModeRef.current === 'survey') void useGlobalViewStore.getState().loadGeometry(project.file_path);
        } else {
          onProjectSelectRef.current(project.file_path);
        }
      });

      // Double-click handler
      el.addEventListener('dblclick', (e) => {
        e.stopPropagation();
        onProjectOpenRef.current(project.file_path);
      });

      // Context menu handler (right-click) - use ref to avoid stale closures
      el.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        e.stopPropagation();

        // Hide tooltip while context menu is open
        contextMenuOpenRef.current = true;
        if (markerTooltipRef.current) {
          markerTooltipRef.current.style.display = 'none';
        }

        onContextMenuRef.current?.(project.file_path, e.clientX, e.clientY);
      });

      // Touch long-press → context menu
      let markerLongPressTimer: ReturnType<typeof setTimeout> | null = null;
      let markerTouchStart: { x: number; y: number } | null = null;
      el.addEventListener('touchstart', (e) => {
        if (e.touches.length !== 1) return;
        const touch = e.touches[0]!;
        markerTouchStart = { x: touch.clientX, y: touch.clientY };
        markerLongPressTimer = setTimeout(() => {
          markerLongPressTimer = null;
          if (!markerTouchStart) return;
          contextMenuOpenRef.current = true;
          if (markerTooltipRef.current) {
            markerTooltipRef.current.style.display = 'none';
          }
          onContextMenuRef.current?.(project.file_path, markerTouchStart.x, markerTouchStart.y);
          markerTouchStart = null;
        }, 500);
      }, { passive: false });
      el.addEventListener('touchmove', (e) => {
        if (!markerTouchStart || !markerLongPressTimer) return;
        const touch = e.touches[0];
        if (!touch) return;
        const dx = touch.clientX - markerTouchStart.x;
        const dy = touch.clientY - markerTouchStart.y;
        if (dx * dx + dy * dy > 100) {
          clearTimeout(markerLongPressTimer);
          markerLongPressTimer = null;
          markerTouchStart = null;
        }
      }, { passive: false });
      el.addEventListener('touchend', () => {
        if (markerLongPressTimer) {
          clearTimeout(markerLongPressTimer);
          markerLongPressTimer = null;
        }
        markerTouchStart = null;
      });
      el.addEventListener('touchcancel', () => {
        if (markerLongPressTimer) {
          clearTimeout(markerLongPressTimer);
          markerLongPressTimer = null;
        }
        markerTouchStart = null;
      });

      // Set initial visibility based on current view mode (via ref, not dep)
      // In survey mode, hide markers for systems whose geometry has loaded successfully
      if (viewModeRef.current === 'survey' && loadingState === 'loaded') {
        el.style.display = 'none';
      }

      // Create and add marker
      const marker = new maplibregl.Marker({ element: el })
        .setLngLat([lng, lat])
        .addTo(map.current!);

      // Store project info for tooltip (avoids closure over stale data)
      el.dataset.projectName = project.name;
      el.dataset.totalLength = String(project.total_length ?? 0);
      el.dataset.stationCount = String(project.station_count ?? 0);
      el.dataset.loadingState = loadingState;

      markersRef.current.set(project.file_path, marker);
    }
  }, [gpsProjects, mapLoaded, resolvedTheme, t, geometryStatesRef, onContextMenuRef, onProjectOpenRef, onProjectSelectRef, precisionRef, selectedProjectIdRef, viewModeRef]);

  // Update marker styles when selection, loading states, or theme changes:
  // colours, border, the loading spinner and the loading-state data attribute.
  useEffect(() => {
    const strokeColor = getThemeColor('--cc-stroke-color');
    const strokeLight = getThemeColor('--cc-stroke-light');
    const errorColor = getThemeColor('--cc-error');
    const shadow = getThemeColor('--cc-map-ctrl-shadow');

    markersRef.current.forEach((marker, filePath) => {
      const el = marker.getElement();
      const inner = el.querySelector('.marker-inner') as HTMLElement | null;
      if (!inner) return;

      const isSelected = selectedProjectId === filePath;
      const loadingState = geometryStates.get(filePath)?.state ?? 'idle';

      // Use same color as survey mode, with error override
      const systemColor = loadingState === 'error'
        ? errorColor
        : generateSystemColor(filePath);

      inner.style.backgroundColor = systemColor;
      inner.style.borderColor = isSelected ? strokeColor : strokeLight;
      inner.style.boxShadow = isSelected
        ? `0 0 0 3px ${strokeLight}, 0 2px 4px ${shadow}`
        : `0 2px 4px ${shadow}`;

      const spinner = inner.querySelector('.marker-spinner') as HTMLElement | null;
      if (loadingState !== 'loading') {
        spinner?.remove();
      } else if (spinner) {
        spinner.style.borderColor = strokeLight;
        spinner.style.borderTopColor = strokeColor;
      } else {
        inner.appendChild(createMarkerSpinner(strokeLight, strokeColor));
      }
      el.dataset.loadingState = loadingState;
    });
  }, [selectedProjectId, geometryStates, resolvedTheme]);

  // Sync marker visibility when view mode or geometry states change
  // In survey mode, hide markers for systems whose geometry has loaded successfully.
  // In markers mode, all markers are visible.
  // This effect only toggles display; colors and spinners are handled by creation/styles effects.
  useEffect(() => {
    if (!mapLoaded) return;

    markersRef.current.forEach((marker, filePath) => {
      const el = marker.getElement();
      const loadingState = geometryStates.get(filePath)?.state ?? 'idle';

      if (viewMode === 'survey' && loadingState === 'loaded') {
        el.style.display = 'none';
      } else {
        el.style.display = 'flex';
      }
    });
  }, [viewMode, geometryStates, mapLoaded]);

  // Fit to bounds button handler
  // Per spec: "Fit All button works regardless of persisted state" - it overrides temporarily
  // so we suppress the next persist to avoid saving this programmatic camera change
  const fitToBounds = useCallback(() => {
    if (!map.current || gpsProjects.length === 0) return;
    // Suppress next persist so Fit All doesn't overwrite user's persisted position
    suppressNextPersistRef.current = true;
    const bounds = calculateBounds();
    map.current.fitBounds(bounds, { padding: 50, maxZoom: 15 });
  }, [calculateBounds, gpsProjects.length]);

  // Handle zoom to specific system when zoomToSystem prop changes
  useEffect(() => {
    if (!map.current || !mapLoaded || !zoomToSystem) return;

    // Find the project to zoom to
    const project = gpsProjects.find(p => p.file_path === zoomToSystem);
    if (!project) {
      // Project not found, still clear the zoom request
      onZoomHandled?.();
      return;
    }

    // Get bounds from project if available, otherwise use center point
    if (project.gps_bounds) {
      const bounds = new maplibregl.LngLatBounds(
        [project.gps_bounds.west, project.gps_bounds.south],
        [project.gps_bounds.east, project.gps_bounds.north]
      );
      map.current.fitBounds(bounds, { padding: 100, maxZoom: 16 });
    } else if (project.gps_center) {
      // Zoom to center point with a reasonable zoom level
      map.current.flyTo({
        center: project.gps_center,
        zoom: 15,
        duration: 500,
      });
    }

    // Signal that zoom was handled so parent can clear the request
    onZoomHandled?.();
  }, [zoomToSystem, mapLoaded, gpsProjects, onZoomHandled]);

  if (gpsProjects.length === 0) {
    return (
      <div
        data-testid="globalview-map"
        className="h-full flex items-center justify-center bg-panel text-text-secondary"
      >
        <div className="text-center">
          <div className="text-4xl mb-4">&#127758;</div>
          <p>{t('globalView.map.empty.title')}</p>
          <p className="text-sm mt-2">
            {t('globalView.map.empty.subtitle')}
          </p>
        </div>
      </div>
    );
  }

  return (
    <div data-testid="globalview-map" className="h-full relative">
      <div ref={mapContainer} className="w-full h-full" />
      {webgl2Unavailable && <MapUnavailable testId="globalview-webgl2-unavailable" />}

      {/* Fit to bounds button */}
      <button
        onClick={fitToBounds}
        data-testid="globalview-map-fit-all"
        className="absolute top-2 left-2 bg-panel border border-border rounded px-2 py-1 text-sm hover:bg-background"
        title={t('globalView.map.fitAllTitle')}
      >
        {t('globalView.map.fitAll')}
      </button>

      {/* View mode toggle - positioned below navigation controls */}
      <div className="absolute top-24 right-2">
        <ViewModeToggle
          mode={viewMode}
          onModeChange={handleViewModeChange}
        />
      </div>

      {/* Imagery source names. Unconditional here: unlike the survey map, the
          satellite layer IS this map's base layer, so the attribution is owed
          for as long as the view exists. Bottom-flush on the right, with the
          project count lifted above it - Esri requires the names not be
          obstructed, and the count chip previously owned this corner. */}
      <MapAttribution
        visible
        className="absolute bottom-0 right-0 z-10"
        elementRef={attributionParseIndicatorClearance}
      />

      {/* Project count indicator */}
      <div
        ref={systemCountParseIndicatorClearance}
        className="absolute bottom-8 right-2 bg-panel-90 border border-border rounded px-2 py-1 text-xs"
      >
        {t('globalView.map.systemCount', {
          count: gpsProjects.length,
          countFormatted: formatNumber(gpsProjects.length),
        })}
      </div>
    </div>
  );
});

/**
 * Owns the map's generation: "Reload map" on the lost-WebGL-context toast bumps
 * it, which remounts the map - map.remove() on the dead context, then a fresh map
 * (new canvas, new context) from the same props (#576 D38-LOST).
 */
const GlobalViewMap = forwardRef<GlobalViewMapHandle, GlobalViewMapProps>(function GlobalViewMapHost(props, ref) {
  const [generation, setGeneration] = useState(0);
  const reloadMap = useCallback(() => setGeneration((g) => g + 1), []);
  return <GlobalViewMapInner key={generation} ref={ref} {...props} onReloadMap={reloadMap} />;
});

export default GlobalViewMap;
