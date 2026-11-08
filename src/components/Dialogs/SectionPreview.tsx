import { useEffect, useRef, useState, useMemo } from 'react';
import * as maplibregl from 'maplibre-gl';
import type { DmpSection, DmpShot } from '../../types/commands';
import type { Section, Station, Shot } from '../../types/project';
import { metersToMapCoords, getBounds } from '../../utils/coordinates';
import { circularMeanDegrees } from '../../utils/angles';
import { useUIStore } from '../../store/uiStore';
import { useNetworkStore } from '../../store/networkStore';
import { getMapThemeColors, getThemeColor } from '../../utils/themeColors';
import { getTileConfig } from '../../utils/tileConfig';
import { attachDragReleaseForwarder } from '../../utils/mapDragRelease';
import { MapAttribution } from '../Map/MapAttribution';
import { createMap } from '../Map/createMap';
import { MapUnavailable } from '../Map/MapUnavailable';
import type { GpsOrigin } from '../../types/config';

/**
 * Merge preview configuration for "From Existing" placement mode.
 * Shows both sections positioned correctly with a connection line.
 */
export interface MergePreviewConfig {
  /** The existing/target section to merge with */
  targetSection: Section;
  /** Station index in the current (import) section to merge */
  importStationIndex: number;
  /** Station index in the target section to merge */
  targetStationIndex: number;
}

/**
 * Merge preview for import-to-import connections (target is a DMP section not yet in the project).
 */
export interface DmpMergePreviewConfig {
  /** The target DMP section (not yet imported) */
  targetDmpSection: DmpSection;
  /** Color for the target section */
  targetColor: string;
  /** Station index in the current (import) section to merge */
  importStationIndex: number;
  /** Station index in the target DMP section to merge */
  targetStationIndex: number;
}

/**
 * Geometry for a context section (other sections in the same segment).
 * Rendered behind the main preview sections to show surrounding cave context.
 */
export interface ContextSectionGeometry {
  shots: Array<{ from: [number, number]; to: [number, number] }>;
  color: string;
}

/**
 * A step that is not a number contributes nothing.
 *
 * A single NaN here does not spoil one station, it spoils EVERY station after
 * it: each is computed from the last, so one unmeasured shot emptied the whole
 * preview centerline and handed `fitBounds` NaNs. Unmeasured rows are filtered
 * out before they reach a preview (`isEmittedShot`), so this is the backstop for
 * whatever the next source of a half-filled shot turns out to be - the geometry
 * downstream of the gap is still worth drawing.
 */
const finiteStep = (value: number): number => (Number.isFinite(value) ? value : 0);

/**
 * Compute station coordinates from DMP shots.
 * Each station is positioned relative to the previous one based on shot heading and length.
 * Depth change uses depth_in/out to stay consistent with placement overlay behavior.
 */
export function computeStationsFromDmpShots(shots: DmpShot[]): Array<{ x: number; y: number; z: number }> {
  // Start at origin
  const stations: Array<{ x: number; y: number; z: number }> = [{ x: 0, y: 0, z: 0 }];

  for (const shot of shots) {
    if (shot.shot_type !== 'Standard') continue;

    const prev = stations[stations.length - 1];
    if (!prev) continue;

    // Use circular mean for heading to handle wrap-around correctly
    const azimuth = circularMeanDegrees(shot.heading_in, shot.heading_out);
    const azimuthRad = (azimuth * Math.PI) / 180;

    const depthChange = shot.depth_out - shot.depth_in;

    // Compute next station position. A missing length, heading or depth leaves
    // the station where the previous one is rather than poisoning the rest of
    // the section - the shot still gets a station, so indices keep matching the
    // rows they came from.
    const x = prev.x + finiteStep(shot.length * Math.sin(azimuthRad));
    const y = prev.y + finiteStep(shot.length * Math.cos(azimuthRad));
    const z = prev.z + finiteStep(depthChange);

    stations.push({ x, y, z });
  }

  return stations;
}

/**
 * Compute station coordinates from project Section shots.
 * Used for existing sections in merge preview.
 * Depth change uses depth_in/out to stay consistent with placement overlay behavior.
 */
export function computeStationsFromShots(shots: Shot[]): Array<{ x: number; y: number; z: number }> {
  // Start at origin
  const stations: Array<{ x: number; y: number; z: number }> = [{ x: 0, y: 0, z: 0 }];

  for (const shot of shots) {
    const prev = stations[stations.length - 1];
    if (!prev) continue;

    // Use circular mean for heading to handle wrap-around correctly
    const azimuth = circularMeanDegrees(shot.azimuth_in, shot.azimuth_out);
    const azimuthRad = (azimuth * Math.PI) / 180;

    const depthChange = shot.depth_out - shot.depth_in;

    // Compute next station position
    const x = prev.x + shot.length * Math.sin(azimuthRad);
    const y = prev.y + shot.length * Math.cos(azimuthRad);
    const z = prev.z + depthChange;

    stations.push({ x, y, z });
  }

  return stations;
}

/** Segment section for multi-section display in manual_placing phase. */
export interface SegmentSectionEntry {
  section: DmpSection;
  color: string;
  sectionIndex: number;     // DMP file index
  offsetX: number;          // meters from segment root station 0
  offsetY: number;
}

interface SectionPreviewProps {
  /** The section to preview. When null (and no segmentSections), shows empty state. */
  section?: DmpSection | null;
  /** Color for the section lines and stations */
  color?: string;
  /** Existing station to connect to (for "From Existing Station" placement mode) - legacy prop */
  existingStation?: Station | null;
  /** Which station of the import to connect: 'first' or 'last' - legacy prop */
  importStationIndex?: 'first' | 'last' | null;
  /** Merge preview configuration - shows both sections with proper alignment */
  mergePreview?: MergePreviewConfig | null;
  /** Merge preview for import-to-import connections (DMP section target) */
  dmpMergePreview?: DmpMergePreviewConfig | null;
  /** Callback when a station dot is clicked */
  onStationClick?: (stationIndex: number, sectionIndex?: number) => void;
  /** Index of the currently selected station (shows highlight ring) */
  selectedStationIndex?: number | null;
  /** Size of the preview container in pixels (default 250). Its WIDTH, and its
   *  height too unless `height` overrides it. */
  size?: number;
  /** Height of the preview container in pixels when it is not square (the
   *  wizards' map column fills its panel, #567). Everything below already works
   *  per axis - `fitBounds` pads by 10% of `clientWidth` and `clientHeight`
   *  separately - so a rectangular viewport just shows more cave. */
  height?: number;
  /** GPS origin for real-world positioning. When set, enables satellite tiles and positions geometry at real coordinates. */
  gpsOrigin?: GpsOrigin | null;
  /** Station index to offset the gpsOrigin to (default 0). The section is positioned so this station sits at gpsOrigin. */
  gpsStationIndex?: number;
  /** When set with mergePreview, fit bounds focus around this station index in the import section rather than fitting all stations. */
  focusStationIndex?: number | null;
  /** Set of visible-shot indices (0-based among non-deleted shots) that should render at reduced opacity (reference shots). */
  referenceIndices?: Set<number>;
  /** Context sections from the same segment, rendered as dimmed lines behind the main preview. */
  contextSections?: ContextSectionGeometry[] | null;
  /** Multiple sections for segment display (mutually exclusive with section prop). */
  segmentSections?: SegmentSectionEntry[] | null;
  /** Section index of the currently selected anchor station (for segment display). */
  selectedSectionIndex?: number | null;
}

/**
 * A minimal MapLibre preview component for showing section shape.
 * - Configurable size: square at `size` (default 250), or `size` x `height`
 * - Station dots + shot lines
 * - Zoom (wheel, 0.25x to 4x) + pan (drag)
 * - Click on station to select (optional callback)
 * - Selected station shows highlight ring
 * - Auto-fits bounds when the section changes (NOT on resize: a bigger box
 *   shows more cave at the same zoom, it does not rescale the drawing)
 * - No labels (minimal view)
 * - Shows merge preview with both sections when mergePreview is provided
 * - Shows connection line when existingStation is provided (legacy mode)
 */
export default function SectionPreview({
  section,
  color = getThemeColor('--cc-accent'),
  existingStation,
  importStationIndex,
  mergePreview,
  dmpMergePreview,
  onStationClick,
  selectedStationIndex,
  size = 250,
  height = size,
  gpsOrigin,
  gpsStationIndex = 0,
  focusStationIndex,
  referenceIndices,
  contextSections,
  segmentSections,
  selectedSectionIndex,
}: SectionPreviewProps) {
  // When segmentSections is provided, use the first section as the primary
  const effectiveSection = section ?? (segmentSections?.[0]?.section ?? null);
  const effectiveColor = section ? color : (segmentSections?.[0]?.color ?? color);
  const containerRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const [mapLoaded, setMapLoaded] = useState(false);
  const [webgl2Unavailable, setWebgl2Unavailable] = useState(false);
  const onStationClickRef = useRef(onStationClick);
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);

  // Keep ref updated
  useEffect(() => {
    onStationClickRef.current = onStationClick;
  }, [onStationClick]);

  // Compute stations from shots for the current (import) section
  const stations = useMemo(() => {
    const sec = effectiveSection;
    if (!sec) return [];
    const standardShots = sec.shots.filter((s) => s.shot_type === 'Standard');
    return computeStationsFromDmpShots(standardShots);
  }, [effectiveSection]);

  // Compute stations for the target section (for merge preview)
  const targetStations = useMemo(() => {
    if (mergePreview) {
      return computeStationsFromShots(mergePreview.targetSection.shots);
    }
    if (dmpMergePreview) {
      const standardShots = dmpMergePreview.targetDmpSection.shots.filter((s) => s.shot_type === 'Standard');
      return computeStationsFromDmpShots(standardShots);
    }
    return [];
  }, [mergePreview, dmpMergePreview]);

  // Compute offset so the GPS station sits at the GPS origin (local 0,0)
  const gpsStationOffset = useMemo(() => {
    if (!gpsOrigin || stations.length === 0) return { x: 0, y: 0 };
    const station = stations[gpsStationIndex];
    if (!station) return { x: 0, y: 0 };
    return { x: -station.x, y: -station.y };
  }, [gpsOrigin, stations, gpsStationIndex]);

  // Initialize MapLibre instance - container is always mounted
  useEffect(() => {
    if (!containerRef.current || mapRef.current) return;

    // Base zoom level 14 represents 1x
    // Allow zooming out to 0.25x and in much further for detail
    const baseZoom = 14;
    const minZoom = baseZoom - 2; // 0.25x (zoom level 12)
    const maxZoom = 22; // Maximum zoom for detailed inspection

    const initialThemeColors = getMapThemeColors();
    const map = createMap({
      container: containerRef.current,
      style: {
        version: 8,
        sources: {},
        layers: [
          {
            id: 'background',
            type: 'background',
            paint: {
              'background-color': initialThemeColors.previewBg,
            },
          },
        ],
      },
      center: [0, 0],
      zoom: baseZoom,
      minZoom,
      maxZoom,
      attributionControl: false,
      interactive: true,
    });
    if (!map) {
      setWebgl2Unavailable(true);
      return;
    }

    // Store map ref immediately for cleanup support
    mapRef.current = map;

    // Disable rotation
    map.dragRotate.disable();
    map.touchPitch.disable();
    // Two-finger twist rotates by default; the preview is a top-down plan, so a
    // pinch must zoom only.
    map.touchZoomRotate.disableRotation();

    // The preview is a small map inside a dialog, so a pan is easily released
    // over the dialog chrome around it - which would leave it stuck "moving";
    // src/utils/mapDragRelease.ts has why MapLibre needs the help.
    const detachDragReleaseForwarder = attachDragReleaseForwarder(map);

    map.on('load', () => {
      // Check if map was removed during async load
      if (!mapRef.current) return;

      // Add sources
      map.addSource('preview-shots', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });

      map.addSource('preview-stations', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });

      map.addSource('preview-connection', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });

      map.addSource('preview-existing-station', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });

      map.addSource('preview-selected-station', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });

      // Source for context sections (other sections in the segment)
      map.addSource('preview-context-shots', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });

      // Sources for merge preview
      map.addSource('preview-target-shots', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });

      map.addSource('preview-target-stations', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });

      map.addSource('preview-merge-highlight', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });

      // Add context section shot lines layer (furthest back, very dimmed)
      map.addLayer({
        id: 'preview-context-shots-layer',
        type: 'line',
        source: 'preview-context-shots',
        paint: {
          'line-color': ['get', 'color'],
          'line-width': 1,
          'line-opacity': 0.3,
        },
      });

      // Add target section shot lines layer (dimmed, rendered first so it's behind)
      map.addLayer({
        id: 'preview-target-shots-layer',
        type: 'line',
        source: 'preview-target-shots',
        paint: {
          'line-color': initialThemeColors.grayLine,
          'line-width': 1.5,
          'line-opacity': 0.6,
        },
      });

      // Add target section stations layer (dimmed)
      map.addLayer({
        id: 'preview-target-stations-layer',
        type: 'circle',
        source: 'preview-target-stations',
        paint: {
          'circle-radius': 3,
          'circle-color': initialThemeColors.grayLine,
          'circle-stroke-width': 1,
          'circle-stroke-color': initialThemeColors.strokeColor,
          'circle-opacity': 0.6,
        },
      });

      // Add shot lines layer
      map.addLayer({
        id: 'preview-shots-layer',
        type: 'line',
        source: 'preview-shots',
        paint: {
          'line-color': ['get', 'color'],
          'line-width': 2,
          'line-opacity': ['get', 'opacity'],
        },
      });

      // Add connection line layer (dashed line)
      map.addLayer({
        id: 'preview-connection-layer',
        type: 'line',
        source: 'preview-connection',
        paint: {
          'line-color': initialThemeColors.zeroDatum,
          'line-width': 2,
          'line-dasharray': [4, 4],
        },
      });

      // Add stations layer
      map.addLayer({
        id: 'preview-stations-layer',
        type: 'circle',
        source: 'preview-stations',
        paint: {
          'circle-radius': 4,
          'circle-color': ['get', 'color'],
          'circle-stroke-width': 1,
          'circle-stroke-color': initialThemeColors.strokeColor,
        },
      });

      // Add existing station layer (different color)
      map.addLayer({
        id: 'preview-existing-station-layer',
        type: 'circle',
        source: 'preview-existing-station',
        paint: {
          'circle-radius': 6,
          'circle-color': initialThemeColors.zeroDatum,
          'circle-stroke-width': 2,
          'circle-stroke-color': initialThemeColors.strokeColor,
        },
      });

      // Add selected station highlight layer (ring around selected station)
      map.addLayer({
        id: 'preview-selected-station-layer',
        type: 'circle',
        source: 'preview-selected-station',
        paint: {
          'circle-radius': 8,
          'circle-color': 'transparent',
          'circle-stroke-width': 3,
          'circle-stroke-color': initialThemeColors.compassColor,
        },
      });

      // Add merge highlight layer (highlighted merge stations)
      map.addLayer({
        id: 'preview-merge-highlight-layer',
        type: 'circle',
        source: 'preview-merge-highlight',
        paint: {
          'circle-radius': 7,
          'circle-color': initialThemeColors.selectHighlight,
          'circle-stroke-width': 2,
          'circle-stroke-color': initialThemeColors.compassColor,
        },
      });

      // Cursor changes to pointer on station hover
      map.on('mouseenter', 'preview-stations-layer', () => {
        map.getCanvas().style.cursor = 'pointer';
      });

      map.on('mouseleave', 'preview-stations-layer', () => {
        map.getCanvas().style.cursor = '';
      });

      // Click handler for station selection
      map.on('click', 'preview-stations-layer', (e) => {
        const feature = e.features?.[0];
        if (feature) {
          const stationIndex = feature.properties?.stationIndex;
          const sectionIndex = feature.properties?.sectionIndex;
          if (typeof stationIndex === 'number' && onStationClickRef.current) {
            onStationClickRef.current(
              stationIndex,
              typeof sectionIndex === 'number' ? sectionIndex : undefined,
            );
          }
        }
      });

      setMapLoaded(true);
    });

    return () => {
      detachDragReleaseForwarder();
      mapRef.current = null;
      setMapLoaded(false);
      map.remove();
    };
  }, []);

  // Toggle satellite tiles when gpsOrigin changes
  useEffect(() => {
    if (!mapRef.current || !mapLoaded) return;
    const map = mapRef.current;

    if (gpsOrigin) {
      // Add satellite tiles if not already present
      if (!map.getSource('satellite-tiles')) {
        const tileConfig = getTileConfig('esri');
        map.addSource('satellite-tiles', {
          type: 'raster',
          tiles: tileConfig.tiles,
          tileSize: tileConfig.tileSize,
          attribution: tileConfig.attribution,
          minzoom: tileConfig.minZoom,
          maxzoom: tileConfig.maxZoom,
        });
        // Insert satellite below all other layers (above background)
        map.addLayer(
          {
            id: 'satellite-tiles-layer',
            type: 'raster',
            source: 'satellite-tiles',
            paint: { 'raster-opacity': 0.85 },
          },
          'preview-context-shots-layer'
        );
      }
      // Dim the background so satellite shows through
      if (map.getLayer('background')) {
        map.setPaintProperty('background', 'background-opacity', 0);
      }
    } else {
      // Remove satellite tiles
      if (map.getLayer('satellite-tiles-layer')) {
        map.removeLayer('satellite-tiles-layer');
      }
      if (map.getSource('satellite-tiles')) {
        map.removeSource('satellite-tiles');
      }
      // Restore background
      if (map.getLayer('background')) {
        map.setPaintProperty('background', 'background-opacity', 1);
      }
    }
  }, [gpsOrigin, mapLoaded]);

  // Reload stale satellite tiles after offline→online recovery
  const tileReloadGeneration = useNetworkStore((s) => s.tileReloadGeneration);
  useEffect(() => {
    if (!mapRef.current || !mapLoaded || !gpsOrigin || tileReloadGeneration === 0) return;
    const source = mapRef.current.getSource('satellite-tiles');
    if (source && 'clearTiles' in source) {
      (source as any).clearTiles();
      mapRef.current.triggerRepaint();
    }
  }, [tileReloadGeneration, mapLoaded, gpsOrigin]);

  // Update data and fit bounds when section changes
  useEffect(() => {
    if (!mapRef.current || !mapLoaded) return;

    const map = mapRef.current;

    // Calculate offset for merge preview - positions the import section so merge stations align
    // Works for both existing-section mergePreview and DMP dmpMergePreview
    const activePreview = mergePreview ?? dmpMergePreview;
    let importOffset = { x: 0, y: 0 };
    if (activePreview && stations.length > 0 && targetStations.length > 0) {
      const importMergeStation = stations[activePreview.importStationIndex];
      const targetMergeStation = targetStations[activePreview.targetStationIndex];
      if (importMergeStation && targetMergeStation) {
        // Calculate offset to align import station with target station
        importOffset = {
          x: targetMergeStation.x - importMergeStation.x,
          y: targetMergeStation.y - importMergeStation.y,
        };
      }
    }

    // Combined offset: merge preview alignment + GPS station positioning
    const totalOffset = {
      x: importOffset.x + gpsStationOffset.x,
      y: importOffset.y + gpsStationOffset.y,
    };

    // Build shot features for the current (import) section with offset
    const shotFeatures: GeoJSON.Feature[] = [];
    const effectiveSec = effectiveSection;
    const standardShots = effectiveSec?.shots.filter((s) => s.shot_type === 'Standard') ?? [];

    // When using segmentSections, render all sections; otherwise just the primary
    if (segmentSections && segmentSections.length > 0) {
      for (const segEntry of segmentSections) {
        const segStations = computeStationsFromDmpShots(
          segEntry.section.shots.filter((s) => s.shot_type === 'Standard')
        );
        const segStandardShots = segEntry.section.shots.filter((s) => s.shot_type === 'Standard');
        for (let i = 0; i < segStandardShots.length; i++) {
          const from = segStations[i];
          const to = segStations[i + 1];
          if (!from || !to) continue;
          shotFeatures.push({
            type: 'Feature',
            geometry: {
              type: 'LineString',
              coordinates: [
                metersToMapCoords(from.x + segEntry.offsetX, from.y + segEntry.offsetY, gpsOrigin),
                metersToMapCoords(to.x + segEntry.offsetX, to.y + segEntry.offsetY, gpsOrigin),
              ],
            },
            properties: { color: segEntry.color, opacity: 1.0 },
          });
        }
      }
    } else {
      for (let i = 0; i < standardShots.length; i++) {
        const from = stations[i];
        const to = stations[i + 1];
        if (!from || !to) continue;
        shotFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'LineString',
            coordinates: [
              metersToMapCoords(from.x + totalOffset.x, from.y + totalOffset.y, gpsOrigin),
              metersToMapCoords(to.x + totalOffset.x, to.y + totalOffset.y, gpsOrigin),
            ],
          },
          properties: { color: effectiveColor, opacity: referenceIndices?.has(i) ? 0.3 : 1.0 },
        });
      }
    }

    // Build station features with offset for click handling
    let stationFeatures: GeoJSON.Feature[];
    if (segmentSections && segmentSections.length > 0) {
      stationFeatures = [];
      for (const segEntry of segmentSections) {
        const segStations = computeStationsFromDmpShots(
          segEntry.section.shots.filter((s) => s.shot_type === 'Standard')
        );
        for (let i = 0; i < segStations.length; i++) {
          const stn = segStations[i]!;
          stationFeatures.push({
            type: 'Feature',
            geometry: {
              type: 'Point',
              coordinates: metersToMapCoords(stn.x + segEntry.offsetX, stn.y + segEntry.offsetY, gpsOrigin),
            },
            properties: { color: segEntry.color, stationIndex: i, sectionIndex: segEntry.sectionIndex },
          });
        }
      }
    } else {
      stationFeatures = stations.map((station, index) => ({
        type: 'Feature',
        geometry: {
          type: 'Point',
          coordinates: metersToMapCoords(station.x + totalOffset.x, station.y + totalOffset.y, gpsOrigin),
        },
        properties: { color: effectiveColor, stationIndex: index },
      }));
    }

    // Build target section features (for merge preview)
    const targetShotFeatures: GeoJSON.Feature[] = [];
    const targetStationFeatures: GeoJSON.Feature[] = [];
    const mergeHighlightFeatures: GeoJSON.Feature[] = [];
    const connectionFeatures: GeoJSON.Feature[] = [];

    if (activePreview && targetStations.length > 0) {
      // Build target section shot lines
      const targetShotCount = mergePreview
        ? mergePreview.targetSection.shots.length
        : (dmpMergePreview?.targetDmpSection.shots.filter((s) => s.shot_type === 'Standard').length ?? 0);

      for (let i = 0; i < targetShotCount; i++) {
        const from = targetStations[i];
        const to = targetStations[i + 1];
        if (!from || !to) continue;

        targetShotFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'LineString',
            coordinates: [
              metersToMapCoords(from.x, from.y, gpsOrigin),
              metersToMapCoords(to.x, to.y, gpsOrigin),
            ],
          },
          properties: {},
        });
      }

      // Build target section stations
      for (let i = 0; i < targetStations.length; i++) {
        const station = targetStations[i];
        if (!station) continue;
        targetStationFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'Point',
            coordinates: metersToMapCoords(station.x, station.y, gpsOrigin),
          },
          properties: { stationIndex: i },
        });
      }

      // Highlight the merge stations
      const importMergeStation = stations[activePreview.importStationIndex];
      const targetMergeStation = targetStations[activePreview.targetStationIndex];

      if (importMergeStation) {
        mergeHighlightFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'Point',
            coordinates: metersToMapCoords(
              importMergeStation.x + totalOffset.x,
              importMergeStation.y + totalOffset.y,
              gpsOrigin
            ),
          },
          properties: { type: 'import' },
        });
      }

      if (targetMergeStation) {
        mergeHighlightFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'Point',
            coordinates: metersToMapCoords(targetMergeStation.x, targetMergeStation.y, gpsOrigin),
          },
          properties: { type: 'target' },
        });
      }

      // Draw dotted connection line between merge stations
      // Since they should be at the same position after offset, we show a small indicator
      if (importMergeStation && targetMergeStation) {
        connectionFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'LineString',
            coordinates: [
              metersToMapCoords(
                importMergeStation.x + totalOffset.x,
                importMergeStation.y + totalOffset.y,
                gpsOrigin
              ),
              metersToMapCoords(targetMergeStation.x, targetMergeStation.y, gpsOrigin),
            ],
          },
          properties: {},
        });
      }
    }

    // Legacy mode: Build connection line and existing station features
    const existingStationFeatures: GeoJSON.Feature[] = [];

    if (!mergePreview && existingStation && stations.length > 0 && importStationIndex) {
      // Determine which import station to connect
      const importStationCoords =
        importStationIndex === 'first' ? stations[0] : stations[stations.length - 1];

      if (importStationCoords) {
        // For visualization, we'll show a simple connection indicator
        // by placing the existing station at an offset from the import station
        const offsetX = 10; // Show existing station 10m to the side for visibility
        const offsetY = 10;

        const existingX = importStationCoords.x + offsetX;
        const existingY = importStationCoords.y + offsetY;

        // Add connection line
        connectionFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'LineString',
            coordinates: [
              metersToMapCoords(importStationCoords.x, importStationCoords.y, gpsOrigin),
              metersToMapCoords(existingX, existingY, gpsOrigin),
            ],
          },
          properties: {},
        });

        // Add existing station marker
        existingStationFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'Point',
            coordinates: metersToMapCoords(existingX, existingY, gpsOrigin),
          },
          properties: { label: 'Existing' },
        });
      }
    }

    // Build context section shot features
    const contextShotFeatures: GeoJSON.Feature[] = [];
    if (contextSections) {
      for (const ctx of contextSections) {
        for (const shot of ctx.shots) {
          contextShotFeatures.push({
            type: 'Feature',
            geometry: {
              type: 'LineString',
              coordinates: [
                metersToMapCoords(shot.from[0], shot.from[1], gpsOrigin),
                metersToMapCoords(shot.to[0], shot.to[1], gpsOrigin),
              ],
            },
            properties: { color: ctx.color },
          });
        }
      }
    }

    // Update sources
    const contextShotsSource = map.getSource('preview-context-shots');
    if (contextShotsSource && 'setData' in contextShotsSource) {
      (contextShotsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: contextShotFeatures,
      });
    }

    const shotsSource = map.getSource('preview-shots');
    if (shotsSource && 'setData' in shotsSource) {
      (shotsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: shotFeatures,
      });
    }

    const stationsSource = map.getSource('preview-stations');
    if (stationsSource && 'setData' in stationsSource) {
      (stationsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: stationFeatures,
      });
    }

    const connectionSource = map.getSource('preview-connection');
    if (connectionSource && 'setData' in connectionSource) {
      (connectionSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: connectionFeatures,
      });
    }

    const existingStationSource = map.getSource('preview-existing-station');
    if (existingStationSource && 'setData' in existingStationSource) {
      (existingStationSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: existingStationFeatures,
      });
    }

    // Update merge preview sources
    const targetShotsSource = map.getSource('preview-target-shots');
    if (targetShotsSource && 'setData' in targetShotsSource) {
      (targetShotsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: targetShotFeatures,
      });
    }

    const targetStationsSource = map.getSource('preview-target-stations');
    if (targetStationsSource && 'setData' in targetStationsSource) {
      (targetStationsSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: targetStationFeatures,
      });
    }

    const mergeHighlightSource = map.getSource('preview-merge-highlight');
    if (mergeHighlightSource && 'setData' in mergeHighlightSource) {
      (mergeHighlightSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: mergeHighlightFeatures,
      });
    }

    // Build and update selected station highlight feature (with combined offset)
    const selectedStationFeatures: GeoJSON.Feature[] = [];
    if (
      selectedStationIndex !== null &&
      selectedStationIndex !== undefined &&
      selectedStationIndex >= 0
    ) {
      if (segmentSections && selectedSectionIndex != null) {
        // Segment mode: find the station in the segment section
        const segEntry = segmentSections.find(s => s.sectionIndex === selectedSectionIndex);
        if (segEntry) {
          const segStations = computeStationsFromDmpShots(
            segEntry.section.shots.filter((s) => s.shot_type === 'Standard')
          );
          const stn = segStations[selectedStationIndex];
          if (stn) {
            selectedStationFeatures.push({
              type: 'Feature',
              geometry: {
                type: 'Point',
                coordinates: metersToMapCoords(stn.x + segEntry.offsetX, stn.y + segEntry.offsetY, gpsOrigin),
              },
              properties: {},
            });
          }
        }
      } else if (selectedStationIndex < stations.length) {
        const selectedStation = stations[selectedStationIndex];
        if (selectedStation) {
          selectedStationFeatures.push({
            type: 'Feature',
            geometry: {
              type: 'Point',
              coordinates: metersToMapCoords(
                selectedStation.x + totalOffset.x,
                selectedStation.y + totalOffset.y,
                gpsOrigin
              ),
            },
            properties: {},
          });
        }
      }
    }

    const selectedStationSource = map.getSource('preview-selected-station');
    if (selectedStationSource && 'setData' in selectedStationSource) {
      (selectedStationSource as maplibregl.GeoJSONSource).setData({
        type: 'FeatureCollection',
        features: selectedStationFeatures,
      });
    }

    // Fit bounds if we have stations - do this after a small delay to ensure
    // the container has proper dimensions and the map is fully rendered
    const hasStations = stations.length > 0 || (segmentSections && segmentSections.length > 0);
    if (hasStations) {
      const doFitBounds = () => {
        let bounds: { minX: number; maxX: number; minY: number; maxY: number };

        // When focusStationIndex is set with mergePreview, zoom to the overlap area
        if (focusStationIndex != null && activePreview && targetStations.length > 0) {
          const focusStation = stations[focusStationIndex];
          if (focusStation) {
            const focusX = focusStation.x + totalOffset.x;
            const focusY = focusStation.y + totalOffset.y;
            const minRadius = 30; // meters minimum around connection

            // Gather nearby stations from both sections within radius
            const offsetImportStations = stations.map((s) => ({
              x: s.x + totalOffset.x,
              y: s.y + totalOffset.y,
            }));

            const nearby: Array<{ x: number; y: number }> = [];
            for (const s of offsetImportStations) {
              const dist = Math.hypot(s.x - focusX, s.y - focusY);
              if (dist <= minRadius * 1.5) nearby.push(s);
            }
            for (const s of targetStations) {
              const dist = Math.hypot(s.x - focusX, s.y - focusY);
              if (dist <= minRadius * 1.5) nearby.push(s);
            }

            // If too few stations nearby, include ±5 nearest from each section
            if (nearby.length < 3) {
              const sortedImport = [...offsetImportStations]
                .map((s) => ({ ...s, dist: Math.hypot(s.x - focusX, s.y - focusY) }))
                .sort((a, b) => a.dist - b.dist)
                .slice(0, 5);
              const sortedTarget = [...targetStations]
                .map((s) => ({ x: s.x, y: s.y, dist: Math.hypot(s.x - focusX, s.y - focusY) }))
                .sort((a, b) => a.dist - b.dist)
                .slice(0, 5);
              for (const s of [...sortedImport, ...sortedTarget]) {
                if (!nearby.some((n) => n.x === s.x && n.y === s.y)) {
                  nearby.push(s);
                }
              }
            }

            if (nearby.length > 0) {
              const nearbyBounds = getBounds(nearby.map((s) => ({ ...s, z: 0 })));
              // Ensure minimum extent of minRadius
              const cx = (nearbyBounds.minX + nearbyBounds.maxX) / 2;
              const cy = (nearbyBounds.minY + nearbyBounds.maxY) / 2;
              const halfW = Math.max((nearbyBounds.maxX - nearbyBounds.minX) / 2, minRadius);
              const halfH = Math.max((nearbyBounds.maxY - nearbyBounds.minY) / 2, minRadius);
              bounds = {
                minX: cx - halfW,
                maxX: cx + halfW,
                minY: cy - halfH,
                maxY: cy + halfH,
              };
            } else {
              // Fallback: center on focus point with minimum radius
              bounds = {
                minX: focusX - minRadius,
                maxX: focusX + minRadius,
                minY: focusY - minRadius,
                maxY: focusY + minRadius,
              };
            }
          } else {
            // focusStation not found, fall through to default bounds
            bounds = computeDefaultBounds();
          }
        } else {
          bounds = computeDefaultBounds();
        }

        function computeDefaultBounds() {
          let b: { minX: number; maxX: number; minY: number; maxY: number };

          // When segment sections are provided, compute bounds from all sections
          if (segmentSections && segmentSections.length > 0) {
            const allPoints: Array<{ x: number; y: number; z: number }> = [];
            for (const segEntry of segmentSections) {
              const segStations = computeStationsFromDmpShots(
                segEntry.section.shots.filter((s) => s.shot_type === 'Standard')
              );
              for (const stn of segStations) {
                allPoints.push({ x: stn.x + segEntry.offsetX, y: stn.y + segEntry.offsetY, z: stn.z });
              }
            }
            b = allPoints.length > 0 ? getBounds(allPoints) : { minX: -10, maxX: 10, minY: -10, maxY: 10 };
          } else {
            // Get bounds for import section (with combined offset applied)
            const offsetStations = stations.map((s) => ({
              x: s.x + totalOffset.x,
              y: s.y + totalOffset.y,
              z: s.z,
            }));
            b = getBounds(offsetStations);
          }

          // Include target section in bounds if merge preview is active
          if (activePreview && targetStations.length > 0) {
            const targetBounds = getBounds(targetStations);
            b.minX = Math.min(b.minX, targetBounds.minX);
            b.maxX = Math.max(b.maxX, targetBounds.maxX);
            b.minY = Math.min(b.minY, targetBounds.minY);
            b.maxY = Math.max(b.maxY, targetBounds.maxY);
          }

          // Include context sections in bounds
          if (contextSections) {
            for (const ctx of contextSections) {
              for (const shot of ctx.shots) {
                b.minX = Math.min(b.minX, shot.from[0], shot.to[0]);
                b.maxX = Math.max(b.maxX, shot.from[0], shot.to[0]);
                b.minY = Math.min(b.minY, shot.from[1], shot.to[1]);
                b.maxY = Math.max(b.maxY, shot.from[1], shot.to[1]);
              }
            }
          }

          // Include existing station in bounds if present (legacy mode)
          if (!mergePreview && existingStationFeatures.length > 0) {
            const importStation =
              importStationIndex === 'first' ? stations[0] : stations[stations.length - 1];
            if (importStation) {
              const existingX = importStation.x + 10;
              const existingY = importStation.y + 10;
              b.minX = Math.min(b.minX, existingX);
              b.maxX = Math.max(b.maxX, existingX);
              b.minY = Math.min(b.minY, existingY);
              b.maxY = Math.max(b.maxY, existingY);
            }
          }

          return b;
        }

        // Check for degenerate bounds (single point or very small extent)
        const extentX = bounds.maxX - bounds.minX;
        const extentY = bounds.maxY - bounds.minY;
        const minExtent = 1; // 1 meter minimum extent

        if (extentX < minExtent && extentY < minExtent) {
          // Single point or very small area - use setCenter with fixed zoom
          const centerX = (bounds.minX + bounds.maxX) / 2;
          const centerY = (bounds.minY + bounds.maxY) / 2;
          map.jumpTo({
            center: metersToMapCoords(centerX, centerY, gpsOrigin),
            zoom: gpsOrigin ? 18 : 16,
          });
        } else {
          const sw = metersToMapCoords(bounds.minX, bounds.minY, gpsOrigin);
          const ne = metersToMapCoords(bounds.maxX, bounds.maxY, gpsOrigin);

          // Calculate 10% padding based on container size
          const container = map.getContainer();
          const paddingX = Math.round(container.clientWidth * 0.1);
          const paddingY = Math.round(container.clientHeight * 0.1);

          // Add padding and fit (no animation for initial load)
          map.fitBounds([sw, ne], {
            padding: { top: paddingY, bottom: paddingY, left: paddingX, right: paddingX },
            maxZoom: 20, // Don't auto-zoom too close, user can zoom in more if needed
            duration: 0,
          });
        }
      };

      // Use requestAnimationFrame to ensure container has proper dimensions
      requestAnimationFrame(() => {
        if (mapRef.current) {
          doFitBounds();
        }
      });
    }
  }, [effectiveSection, effectiveColor, stations, targetStations, color, mapLoaded, existingStation, importStationIndex, mergePreview, dmpMergePreview, selectedStationIndex, selectedSectionIndex, gpsOrigin, gpsStationOffset, focusStationIndex, referenceIndices, contextSections, segmentSections]);

  // Re-apply theme colors when resolvedTheme changes
  useEffect(() => {
    if (!mapRef.current || !mapLoaded) return;
    const map = mapRef.current;
    const colors = getMapThemeColors();

    // Update background color
    if (map.getLayer('background')) {
      map.setPaintProperty('background', 'background-color', colors.previewBg);
    }
    // Context section layer uses per-feature color, no theme update needed
    // Target section layers
    if (map.getLayer('preview-target-shots-layer')) {
      map.setPaintProperty('preview-target-shots-layer', 'line-color', colors.grayLine);
    }
    if (map.getLayer('preview-target-stations-layer')) {
      map.setPaintProperty('preview-target-stations-layer', 'circle-color', colors.grayLine);
      map.setPaintProperty('preview-target-stations-layer', 'circle-stroke-color', colors.strokeColor);
    }
    // Connection line
    if (map.getLayer('preview-connection-layer')) {
      map.setPaintProperty('preview-connection-layer', 'line-color', colors.zeroDatum);
    }
    // Stations
    if (map.getLayer('preview-stations-layer')) {
      map.setPaintProperty('preview-stations-layer', 'circle-stroke-color', colors.strokeColor);
    }
    // Existing station
    if (map.getLayer('preview-existing-station-layer')) {
      map.setPaintProperty('preview-existing-station-layer', 'circle-color', colors.zeroDatum);
      map.setPaintProperty('preview-existing-station-layer', 'circle-stroke-color', colors.strokeColor);
    }
    // Selected station highlight
    if (map.getLayer('preview-selected-station-layer')) {
      map.setPaintProperty('preview-selected-station-layer', 'circle-stroke-color', colors.compassColor);
    }
    // Merge highlight
    if (map.getLayer('preview-merge-highlight-layer')) {
      map.setPaintProperty('preview-merge-highlight-layer', 'circle-color', colors.selectHighlight);
      map.setPaintProperty('preview-merge-highlight-layer', 'circle-stroke-color', colors.compassColor);
    }
  }, [resolvedTheme, mapLoaded]);

  // Always render the container, overlay placeholder when no section
  return (
    <div
      data-testid="section-preview"
      className="relative border border-border rounded overflow-hidden bg-background"
      style={{ width: size, height }}
    >
      {/* Map container is always mounted for proper lifecycle */}
      <div ref={containerRef} className="w-full h-full" data-testid="section-preview-map" />
      {webgl2Unavailable && <MapUnavailable testId="section-preview-webgl2-unavailable" />}

      {/* Imagery attribution, forced compact. This preview is a few hundred
          pixels square and the whole point of it is the geometry underneath, so
          the source names live behind the `i` that Esri's guidance allows for
          exactly this case. The panel is portalled by Tooltip, which is what
          keeps it out of this container's `overflow-hidden`. */}
      <MapAttribution
        visible={!!gpsOrigin}
        variant="compact"
        className="absolute bottom-1 right-1 z-10"
      />

      {/* Overlay placeholder when no section selected */}
      {!effectiveSection && !(segmentSections && segmentSections.length > 0) && (
        <div className="absolute inset-0 flex items-center justify-center bg-background text-text-secondary text-sm">
          Select a section to preview
        </div>
      )}
    </div>
  );
}
