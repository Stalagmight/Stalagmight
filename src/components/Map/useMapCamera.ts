import { useCallback, useEffect, useRef, type MutableRefObject } from 'react';
import type * as maplibregl from 'maplibre-gl';
import type { SectionId, ShotId, Station, StationRef } from '../../types/project';
import type { GpsOrigin } from '../../types/config';
import type { ProjectStore } from '../../store/projectStore';
import type { UIStoreState } from '../../store/uiSelectKeys';
import { isShotKindVisible } from '../../store/displaySelectors';
import { wheelZoomRateForLevels, ZOOM_LEVELS_PER_NOTCH_DEFAULT } from '../../utils/zoomRate';
import { getBounds, metersToMapCoords } from '../../utils/coordinates';
import type SurveyLayer from './SurveyLayer';

interface MapCameraOptions {
  map: MutableRefObject<maplibregl.Map | null>;
  surveyLayerRef: MutableRefObject<SurveyLayer | null>;
  mapLoaded: boolean;
  config: ProjectStore['config'];
  project: ProjectStore['project'];
  /** Measured stations - only `stations.length` gates the fit-to-bounds request. */
  stations: ProjectStore['stations'];
  activeStations: Station[];
  activeSurveyData: ProjectStore['surveyData'];
  gpsOrigin: GpsOrigin | null;
  showShots: boolean;
  showJumps: boolean;
  showReferenceShots: boolean;
  zoomStep: number;
  zoomMax: number;
  requestFitToBounds: UIStoreState['requestFitToBounds'];
  clearFitToBoundsRequest: UIStoreState['clearFitToBoundsRequest'];
  requestZoomToSection: UIStoreState['requestZoomToSection'];
  clearZoomToSectionRequest: UIStoreState['clearZoomToSectionRequest'];
  requestPanToShot: UIStoreState['requestPanToShot'];
  clearPanToShotRequest: UIStoreState['clearPanToShotRequest'];
  requestCenterOnStation: UIStoreState['requestCenterOnStation'];
  clearCenterOnStationRequest: UIStoreState['clearCenterOnStationRequest'];
  requestCenterOnShot: UIStoreState['requestCenterOnShot'];
  clearCenterOnShotRequest: UIStoreState['clearCenterOnShotRequest'];
  requestZoomToShot: UIStoreState['requestZoomToShot'];
  clearZoomToShotRequest: UIStoreState['clearZoomToShotRequest'];
}

/**
 * The survey map's camera: the zoom config (scroll-zoom rate, inverted scroll,
 * zoom limits), the fit / center / pan / zoom-to helpers, and the effects that
 * serve the uiStore camera requests (fit to bounds, zoom to section, pan to /
 * center on / zoom to a shot, center on a station), each with its
 * pending-until-loaded ref.
 *
 * Moved out of MapCanvas.tsx verbatim (#576 D3). MapCanvas calls it after the
 * satellite and tile-reload effects, where the helpers used to be declared, so
 * the request effects keep their place. Two things moved in the effect order:
 * the scroll-zoom effects (only consulted on later wheel events; nothing else
 * touches `scrollZoom`) and the zoom-limit effect, which still runs after the
 * map exists and before the request effects, and which no effect in between
 * reads the camera for (they only register listeners or add / clear the
 * satellite raster). MapCanvas's ghost-preview effect, which used to sit between
 * the center-on-shot and zoom-to-shot effects, now runs after this hook's
 * effects: it only rebuilds the ghost GeoJSON from its data and the GPS origin,
 * and never reads or moves the camera.
 */
export function useMapCamera({
  map,
  surveyLayerRef,
  mapLoaded,
  config,
  project,
  stations,
  activeStations,
  activeSurveyData,
  gpsOrigin,
  showShots,
  showJumps,
  showReferenceShots,
  zoomStep,
  zoomMax,
  requestFitToBounds,
  clearFitToBoundsRequest,
  requestZoomToSection,
  clearZoomToSectionRequest,
  requestPanToShot,
  clearPanToShotRequest,
  requestCenterOnStation,
  clearCenterOnStationRequest,
  requestCenterOnShot,
  clearCenterOnShotRequest,
  requestZoomToShot,
  clearZoomToShotRequest,
}: MapCameraOptions) {
  // Pending section zoom request (stored when map not loaded, processed when it loads)
  const pendingZoomToSectionRef = useRef<number | null>(null);
  // Pending pan-to-shot request (stored when map not loaded, processed when it loads)
  const pendingPanToShotRef = useRef<{ sectionId: number; shotId: number } | null>(null);
  // Pending center-on-station request (stored when map not loaded, processed when it loads)
  const pendingCenterOnStationRef = useRef<StationRef | null>(null);
  // Pending center-on-shot request (stored when map not loaded, processed when it loads)
  const pendingCenterOnShotRef = useRef<{ sectionId: number; shotId: number } | null>(null);

  // Scroll-zoom speed: map.controls.zoom_speed is zoom LEVELS per 100 px wheel
  // notch, and utils/zoomRate.ts inverts MapLibre's sigmoid to the rate that
  // delivers them. Handing the setting over as speed/100 instead made the top
  // of its range inert - the sigmoid is asymptotic at one level per notch, so
  // 3.0 and 5.0 were 0.930 and 0.990 levels, a difference nobody can feel
  // (#429). Shift+wheel still applies MapLibre's 1/4-rate precision modifier.
  //
  // Both MapLibre rates (trackpad AND wheel) are set together, on purpose:
  // MapLibre's per-chain wheel-vs-trackpad classifier misfires on Windows
  // precision trackpads (measured ~70% wheel-classified events on gestures that
  // felt sluggish, and the verdict can flap mid-gesture), and with equal rates
  // its verdict has no visible effect.
  const zoomLevelsPerNotch = config?.map.controls.zoom_speed ?? ZOOM_LEVELS_PER_NOTCH_DEFAULT;
  useEffect(() => {
    if (!mapLoaded || !map.current) return;
    const rate = wheelZoomRateForLevels(zoomLevelsPerNotch);
    map.current.scrollZoom.setZoomRate(rate);
    map.current.scrollZoom.setWheelZoomRate(rate);
  }, [mapLoaded, zoomLevelsPerNotch]);

  // Invert scroll: MapLibre has no public wheel-direction option, but its
  // HandlerManager looks up `scrollZoom.wheel` at dispatch time, so replacing
  // the method on the instance is a stable interception point. The proxy flips
  // deltaY only; every other read (clientX/Y for the zoom anchor, deltaMode,
  // preventDefault, the retained _lastWheelEvent) forwards to the original
  // event, with methods bound so invocation stays legal.
  const invertScroll = config?.map.controls.invert_scroll ?? false;
  useEffect(() => {
    if (!mapLoaded || !map.current || !invertScroll) return;
    const sz = map.current.scrollZoom as unknown as { wheel: (e: WheelEvent) => void };
    const orig = sz.wheel.bind(map.current.scrollZoom);
    sz.wheel = (e: WheelEvent) =>
      orig(
        new Proxy(e, {
          get(target, prop) {
            if (prop === 'deltaY') return -target.deltaY;
            const v = Reflect.get(target, prop);
            return typeof v === 'function' ? v.bind(target) : v;
          },
        }) as WheelEvent
      );
    return () => {
      sz.wheel = orig;
    };
  }, [mapLoaded, invertScroll]);

  // Set map zoom limits from config
  useEffect(() => {
    if (!map.current || !config) return;
    map.current.setMinZoom(config.map.zoom_min ?? 0);
    map.current.setMaxZoom(config.map.zoom_max ?? 22);
  }, [config]);

  // 10% margins on all sides. The map is the central flex column (already
  // sized to exclude the side panels), so no panel offsets are needed - adding
  // them double-counts the panel widths and squeezes the fit (see #64).
  const calcPadding = useCallback((container: HTMLElement) => {
    const marginX = Math.round(container.clientWidth * 0.1);
    const marginY = Math.round(container.clientHeight * 0.1);
    return { top: marginY, bottom: marginY, left: marginX, right: marginX };
  }, []);

  // Fit to survey bounds using transformed coordinates
  const fitToBounds = useCallback((options?: { snap?: boolean }) => {
    // Frame the *active* (drawn) geometry - adjusted in the closure-adjusted view.
    if (!map.current) return;

    // Prefer the SoA `stationCoords` typed array (stride-3 [x,y,z,…]) so the
    // fit computes X/Y bounds without materializing the ~89k-object lazy
    // `activeStations` view - this callback runs on every load via the
    // requestFitToBounds effect. Fall back to `activeStations` only when
    // surveyData isn't available yet (unusual early-init paths).
    let bounds: { minX: number; maxX: number; minY: number; maxY: number };
    const coordsArr = activeSurveyData?.stationCoords;
    if (coordsArr && coordsArr.length > 0) {
      let minX = Infinity;
      let maxX = -Infinity;
      let minY = Infinity;
      let maxY = -Infinity;
      for (let i = 0; i + 2 < coordsArr.length; i += 3) {
        const x = coordsArr[i]!;
        const y = coordsArr[i + 1]!;
        if (x < minX) minX = x;
        if (x > maxX) maxX = x;
        if (y < minY) minY = y;
        if (y > maxY) maxY = y;
      }
      bounds = { minX, maxX, minY, maxY };
    } else {
      if (activeStations.length === 0) return;
      const coords = activeStations.map((s) => s.coordinates);
      bounds = getBounds(coords);
    }

    // Transform bounds to map coordinates
    const sw = metersToMapCoords(bounds.minX, bounds.minY, gpsOrigin);
    const ne = metersToMapCoords(bounds.maxX, bounds.maxY, gpsOrigin);

    // Reset padding to avoid MapLibre accumulation bug
    map.current.setPadding({ top: 0, right: 0, bottom: 0, left: 0 });

    // Calculate 10% padding based on container size (no panel offsets)
    const container = map.current.getContainer();
    const padding = calcPadding(container);

    // Respect the configured animation speed (matches fitToSection / center moves).
    // `snap` forces an instant jump regardless of the setting.
    const animationConfig = config?.map.animation;
    const animationDuration = animationConfig && !animationConfig.enabled ? 0 : (animationConfig?.duration ?? 300);

    // Use MapLibre's fitBounds with percentage-based padding
    map.current.fitBounds([sw, ne], {
      padding,
      maxZoom: 20,
      ...(options?.snap ? { animate: false } : { duration: animationDuration }),
    });
  }, [activeStations, activeSurveyData, gpsOrigin, calcPadding, config]);

  // Fit to a specific section's bounds
  const fitToSection = useCallback(
    (sectionId: number) => {
      if (!map.current || !config) return;

      // Filter stations belonging to this section
      const sectionStations = activeStations.filter((s) => s.station_ref.section_id === sectionId);

      // Empty section: nothing to zoom to
      if (sectionStations.length === 0) {
        return;
      }

      // Fit to *rendered* geometry: endpoints of drawn shots plus zero-datum
      // stations, falling back to all stations when nothing matches. A hidden
      // connector or a station only a hidden shot touches shouldn't inflate the
      // bounds and shrink the section; a *visible* one must be included, or it
      // could be left off-screen. Jumps and References are independent toggles
      // (#27), so this is per-kind - judging both by the reference toggle alone
      // got it backwards in each direction.
      const byIndex = new Map<number, (typeof sectionStations)[number]>();
      for (const s of sectionStations) byIndex.set(s.station_ref.station_index, s);
      const section = project?.sections.find((s) => s.id === sectionId);
      const rendered: typeof sectionStations = [];
      const seenIdx = new Set<number>();
      const includeIdx = (idx: number) => {
        if (seenIdx.has(idx)) return;
        const st = byIndex.get(idx);
        if (st) {
          rendered.push(st);
          seenIdx.add(idx);
        }
      };
      section?.shots.forEach((shot, i) => {
        if (!isShotKindVisible(shot.kind, showShots, showJumps, showReferenceShots)) return;
        includeIdx(i);
        includeIdx(i + 1);
      });
      for (const s of sectionStations) {
        if (s.is_zero_datum) includeIdx(s.station_ref.station_index);
      }
      const fitStations = rendered.length > 0 ? rendered : sectionStations;

      // Stop any running animation before starting a new one
      map.current.stop();

      // Reset padding to avoid MapLibre accumulation bug
      map.current.setPadding({ top: 0, right: 0, bottom: 0, left: 0 });

      // Get animation config
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;

      // Padding: 10% margins only. The map container is the central flex
      // column, so it already excludes the side panels - adding the panel
      // widths here double-counts and squeezes the section into a sliver
      // (e.g. a 730px map minus 250+300px panels left ~34px usable). Matches
      // fitToBounds.
      const container = map.current.getContainer();
      const padding = calcPadding(container);

      // Single station: center at point_zoom level
      if (fitStations.length === 1) {
        const station = fitStations[0]!;
        const mapCoords = metersToMapCoords(station.coordinates.x, station.coordinates.y, gpsOrigin);

        map.current.easeTo({
          center: mapCoords as [number, number],
          zoom: animationConfig.point_zoom,
          duration: animationDuration,
          padding,
        });
        return;
      }

      // Multiple stations: calculate bounds and fit
      const coords = fitStations.map((s) => s.coordinates);
      const bounds = getBounds(coords);

      // Handle zero-area bounds (all stations at same XY) like single-station case
      if (bounds.minX === bounds.maxX && bounds.minY === bounds.maxY) {
        const mapCoords = metersToMapCoords(bounds.minX, bounds.minY, gpsOrigin);
        map.current.easeTo({
          center: mapCoords as [number, number],
          zoom: animationConfig.point_zoom,
          duration: animationDuration,
          padding,
        });
        return;
      }

      // Transform bounds to map coordinates (pass gpsOrigin for satellite alignment)
      const sw = metersToMapCoords(bounds.minX, bounds.minY, gpsOrigin);
      const ne = metersToMapCoords(bounds.maxX, bounds.maxY, gpsOrigin);

      // Always fit the section's bounds. fitBounds depends only on the bounds,
      // padding and container - not the live (possibly mid-animation) camera -
      // so rapidly clicking between different sections can't drift the zoom.
      // (The previous skip-zoom-in heuristic read the transitional camera via
      // map.project() and accumulated across interrupted animations.) Re-fitting
      // an already-framed section is idempotent (same target zoom).
      map.current.fitBounds([sw, ne], {
        padding,
        linear: true,
        duration: animationDuration,
        maxZoom: config.map.zoom_max ?? 22,
      });
    },
    [activeStations, project?.sections, config, gpsOrigin, showShots, showJumps, showReferenceShots, calcPadding]
  );

  // Pan to a shot's midpoint (used when double-clicking a row in DataTable)
  const panToShotMidpoint = useCallback(
    (sectionId: SectionId, shotId: ShotId) => {
      if (!map.current || !config || !project) return;

      // Find the section
      const section = project.sections.find((s) => s.id === sectionId);
      if (!section) return;

      // Find the shot index (shot position - 1, since shots connect stations)
      const shotIndex = section.shots.findIndex((s) => s.id === shotId);
      if (shotIndex === -1) return;

      // Shot at index N connects station_index N (from) to station_index N+1 (to)
      const fromStationIndex = shotIndex;
      const toStationIndex = shotIndex + 1;

      // Find the two stations
      const fromStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === fromStationIndex
      );
      const toStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === toStationIndex
      );

      if (!fromStation || !toStation) return;

      // Calculate midpoint in project coordinates (meters)
      const midpointX = (fromStation.coordinates.x + toStation.coordinates.x) / 2;
      const midpointY = (fromStation.coordinates.y + toStation.coordinates.y) / 2;

      // Convert to map coordinates
      const mapCoords = metersToMapCoords(midpointX, midpointY, gpsOrigin);

      // Get animation config
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;

      // Pan to the midpoint with smooth animation
      map.current.easeTo({
        center: mapCoords as [number, number],
        duration: animationDuration,
      });
    },
    [project, activeStations, config, gpsOrigin]
  );

  // Progressive zoom-in centered on a shot's midpoint: each call recenters and
  // zooms one step closer (capped at zoom_max). Drives the resolve view's zoom
  // button (#57) - clicking repeatedly closes in on the problematic shot.
  const zoomToShotMidpoint = useCallback(
    (sectionId: SectionId, shotId: ShotId) => {
      if (!map.current || !config || !project) return;
      const section = project.sections.find((s) => s.id === sectionId);
      if (!section) return;
      const shotIndex = section.shots.findIndex((s) => s.id === shotId);
      if (shotIndex === -1) return;
      const fromStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === shotIndex,
      );
      const toStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === shotIndex + 1,
      );
      if (!fromStation || !toStation) return;
      const midpointX = (fromStation.coordinates.x + toStation.coordinates.x) / 2;
      const midpointY = (fromStation.coordinates.y + toStation.coordinates.y) / 2;
      const mapCoords = metersToMapCoords(midpointX, midpointY, gpsOrigin);
      const currentZoom = map.current.getZoom();
      const targetZoom = Math.min(zoomMax, currentZoom + zoomStep);
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;
      map.current.easeTo({
        center: mapCoords as [number, number],
        zoom: targetZoom,
        duration: animationDuration,
      });
    },
    [project, activeStations, config, gpsOrigin, zoomMax, zoomStep],
  );

  // Center on a station (used by DetailLink)
  // Returns true if centering was performed, false if prerequisites not met
  const fitToStation = useCallback(
    (stationRef: StationRef): boolean => {
      if (!map.current || !config) return false;

      // Find the station
      const station = activeStations.find(
        (s) =>
          s.station_ref.section_id === stationRef.section_id &&
          s.station_ref.station_index === stationRef.station_index
      );
      if (!station) return false;

      // Stop any running animation before starting a new one
      map.current.stop();

      // Reset padding to avoid MapLibre accumulation bug (follows fitToSection pattern)
      map.current.setPadding({ top: 0, right: 0, bottom: 0, left: 0 });

      // Convert to map coordinates
      const mapCoords = metersToMapCoords(station.coordinates.x, station.coordinates.y, gpsOrigin);

      // Get animation config
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;

      // Center on the station using point_zoom (single point centering, no padding needed)
      map.current.easeTo({
        center: mapCoords as [number, number],
        zoom: animationConfig.point_zoom,
        duration: animationDuration,
      });

      return true;
    },
    [activeStations, config, gpsOrigin]
  );

  // Center on a shot's midpoint (used by DetailLink)
  // Returns true if centering was performed, false if prerequisites not met
  const fitToShot = useCallback(
    (sectionId: SectionId, shotId: ShotId): boolean => {
      if (!map.current || !config || !project) return false;

      // Find the section
      const section = project.sections.find((s) => s.id === sectionId);
      if (!section) return false;

      // Find the shot index
      const shotIndex = section.shots.findIndex((s) => s.id === shotId);
      if (shotIndex === -1) return false;

      // Shot at index N connects station_index N (from) to station_index N+1 (to)
      const fromStationIndex = shotIndex;
      const toStationIndex = shotIndex + 1;

      // Find the two stations
      const fromStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === fromStationIndex
      );
      const toStation = activeStations.find(
        (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === toStationIndex
      );

      if (!fromStation || !toStation) return false;

      // Stop any running animation before starting a new one
      map.current.stop();

      // Reset padding to avoid MapLibre accumulation bug (follows fitToSection pattern)
      map.current.setPadding({ top: 0, right: 0, bottom: 0, left: 0 });

      // Calculate midpoint in project coordinates (meters)
      const midpointX = (fromStation.coordinates.x + toStation.coordinates.x) / 2;
      const midpointY = (fromStation.coordinates.y + toStation.coordinates.y) / 2;

      // Convert to map coordinates
      const mapCoords = metersToMapCoords(midpointX, midpointY, gpsOrigin);

      // Get animation config
      const animationConfig = config.map.animation;
      const animationDuration = animationConfig.enabled ? animationConfig.duration : 0;

      // Center on the shot midpoint using point_zoom (single point centering, no padding needed)
      map.current.easeTo({
        center: mapCoords as [number, number],
        zoom: animationConfig.point_zoom,
        duration: animationDuration,
      });

      return true;
    },
    [project, activeStations, config, gpsOrigin]
  );

  // Fit to bounds when requested (e.g., after import)
  useEffect(() => {
    if (requestFitToBounds && mapLoaded && stations.length > 0) {
      const snap = requestFitToBounds === 'snap';
      if (snap) {
        fitToBounds({ snap: true });
        // The load's FIRST feature build can run against the pre-fit camera;
        // if that transient zoom sat above the label build threshold while
        // the fitted camera lands inside the hysteresis band, ~165k label
        // features would persist that a load at the final camera never
        // builds. Re-evaluate from a clean slate at the fitted camera (#mem).
        surveyLayerRef.current?.resetLabelZoomHysteresis();
        clearFitToBoundsRequest();
      } else {
        // Small delay to ensure data is rendered
        setTimeout(() => {
          fitToBounds();
          // Same clean-slate label re-evaluation as the snap path, but the
          // camera animates here - wait for the map to settle ('idle' also
          // fires when the fit was a no-op, unlike 'moveend').
          map.current?.once('idle', () => {
            surveyLayerRef.current?.resetLabelZoomHysteresis();
          });
          clearFitToBoundsRequest();
        }, 100);
      }
    }
  }, [requestFitToBounds, mapLoaded, stations.length, fitToBounds, clearFitToBoundsRequest]);

  // Fit to section when requested (e.g., from sections panel double-click)
  useEffect(() => {
    // CRITICAL: Clear request immediately (before any early returns) to prevent stuck requests
    if (requestZoomToSection !== null) {
      const sectionId = requestZoomToSection;
      clearZoomToSectionRequest();
      // If map is loaded, process immediately; otherwise store for later
      if (mapLoaded) {
        fitToSection(sectionId);
      } else {
        pendingZoomToSectionRef.current = sectionId;
      }
    }
  }, [requestZoomToSection, mapLoaded, clearZoomToSectionRequest, fitToSection]);

  // Process pending section zoom request when map loads
  useEffect(() => {
    if (mapLoaded && pendingZoomToSectionRef.current !== null) {
      const sectionId = pendingZoomToSectionRef.current;
      pendingZoomToSectionRef.current = null;
      fitToSection(sectionId);
    }
  }, [mapLoaded, fitToSection]);

  // Pan to shot midpoint when requested (e.g., from DataTable double-click)
  useEffect(() => {
    // CRITICAL: Clear request immediately (before any early returns) to prevent stuck requests
    if (requestPanToShot !== null) {
      const { sectionId, shotId } = requestPanToShot;
      clearPanToShotRequest();
      // If map is loaded, process immediately; otherwise store for later
      if (mapLoaded) {
        panToShotMidpoint(sectionId, shotId);
      } else {
        pendingPanToShotRef.current = { sectionId, shotId };
      }
    }
  }, [requestPanToShot, mapLoaded, clearPanToShotRequest, panToShotMidpoint]);

  // Process pending pan-to-shot request when map loads
  useEffect(() => {
    if (mapLoaded && pendingPanToShotRef.current !== null) {
      const { sectionId, shotId } = pendingPanToShotRef.current;
      pendingPanToShotRef.current = null;
      panToShotMidpoint(sectionId, shotId);
    }
  }, [mapLoaded, panToShotMidpoint]);

  // Center on station when requested (e.g., from DetailLink click)
  useEffect(() => {
    // CRITICAL: Clear request immediately (before any early returns) to prevent stuck requests
    if (requestCenterOnStation !== null) {
      const stationRef = requestCenterOnStation;
      clearCenterOnStationRequest();
      // If map is loaded, process immediately; otherwise store for later
      if (mapLoaded) {
        fitToStation(stationRef);
      } else {
        pendingCenterOnStationRef.current = stationRef;
      }
    }
  }, [requestCenterOnStation, mapLoaded, clearCenterOnStationRequest, fitToStation]);

  // Process pending center-on-station request when map loads
  useEffect(() => {
    if (mapLoaded && pendingCenterOnStationRef.current !== null) {
      const stationRef = pendingCenterOnStationRef.current;
      // Only clear pending ref if centering was successful
      if (fitToStation(stationRef)) {
        pendingCenterOnStationRef.current = null;
      }
    }
  }, [mapLoaded, fitToStation]);

  // Center on shot when requested (e.g., from DetailLink click)
  useEffect(() => {
    // CRITICAL: Clear request immediately (before any early returns) to prevent stuck requests
    if (requestCenterOnShot !== null) {
      const { sectionId, shotId } = requestCenterOnShot;
      clearCenterOnShotRequest();
      // If map is loaded, process immediately; otherwise store for later
      if (mapLoaded) {
        fitToShot(sectionId, shotId);
      } else {
        pendingCenterOnShotRef.current = { sectionId, shotId };
      }
    }
  }, [requestCenterOnShot, mapLoaded, clearCenterOnShotRequest, fitToShot]);

  // Progressive zoom-in onto a shot (resolve view's zoom button, #57). Each
  // request recenters and zooms one step closer.
  useEffect(() => {
    if (requestZoomToShot !== null) {
      const { sectionId, shotId } = requestZoomToShot;
      clearZoomToShotRequest();
      if (mapLoaded) zoomToShotMidpoint(sectionId, shotId);
    }
  }, [requestZoomToShot, mapLoaded, clearZoomToShotRequest, zoomToShotMidpoint]);

  // Process pending center-on-shot request when map loads
  useEffect(() => {
    if (mapLoaded && pendingCenterOnShotRef.current !== null) {
      const { sectionId, shotId } = pendingCenterOnShotRef.current;
      // Only clear pending ref if centering was successful
      if (fitToShot(sectionId, shotId)) {
        pendingCenterOnShotRef.current = null;
      }
    }
  }, [mapLoaded, fitToShot]);

  return { fitToBounds };
}
