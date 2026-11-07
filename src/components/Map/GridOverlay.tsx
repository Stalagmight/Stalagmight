import { useEffect, useRef, useCallback } from 'react';
import * as maplibregl from 'maplibre-gl';
import { metersToMapCoords } from '../../utils/coordinates';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { canWriteStyle, hasLiveStyle, BOTTOM_SURVEY_LAYERS } from './mapUtils';
import { getThemeColor } from '../../utils/themeColors';
import type { GpsOrigin } from '../../types/config';

interface GridOverlayProps {
  map: maplibregl.Map;
  gpsOrigin: GpsOrigin | null;
  /**
   * Moves once the style is back after a WebGL context restore (useStyleEpoch):
   * the grid is a style layer, so writes skipped while the style was gone are
   * made again (#576 NF-MAP-5).
   */
  styleEpoch?: number;
}

/** Available grid spacing options in meters */
export const GRID_SPACING_OPTIONS = [1, 5, 10, 25, 50, 100] as const;

/**
 * GridOverlay renders a coordinate grid on the map that scales with the survey data.
 * Grid lines are rendered in map coordinates (meters) and update when the map view changes.
 */
export default function GridOverlay({ map, gpsOrigin, styleEpoch = 0 }: GridOverlayProps) {
  const layerAdded = useRef(false);
  const showGrid = useUIStore((s) => s.showGrid);
  const gridSpacing = useUIStore((s) => s.gridSpacing);
  // Subscribe to resolvedTheme so gridColor re-reads the CSS variable on theme change
  useUIStore((s) => s.resolvedTheme);
  const stations = useProjectStore((s) => s.stations);

  const gridColor = getThemeColor('--cc-grid-default');

  // Generate grid GeoJSON based on current station bounds
  const generateGridGeoJSON = useCallback((): GeoJSON.FeatureCollection => {
    if (stations.length === 0) {
      return { type: 'FeatureCollection', features: [] };
    }

    // Calculate bounds from stations
    let minX = Infinity, maxX = -Infinity;
    let minY = Infinity, maxY = -Infinity;

    for (const station of stations) {
      const { x, y } = station.coordinates;
      if (x < minX) minX = x;
      if (x > maxX) maxX = x;
      if (y < minY) minY = y;
      if (y > maxY) maxY = y;
    }

    // Expand bounds generously so grid extends well beyond the survey
    const width = maxX - minX;
    const height = maxY - minY;
    const padding = Math.max(width, height) * 1.0;
    minX -= padding;
    maxX += padding;
    minY -= padding;
    maxY += padding;

    // Align to grid spacing
    const gridMinX = Math.floor(minX / gridSpacing) * gridSpacing;
    const gridMaxX = Math.ceil(maxX / gridSpacing) * gridSpacing;
    const gridMinY = Math.floor(minY / gridSpacing) * gridSpacing;
    const gridMaxY = Math.ceil(maxY / gridSpacing) * gridSpacing;

    const features: GeoJSON.Feature[] = [];

    // Vertical lines (constant X)
    for (let x = gridMinX; x <= gridMaxX; x += gridSpacing) {
      const start = metersToMapCoords(x, gridMinY, gpsOrigin);
      const end = metersToMapCoords(x, gridMaxY, gpsOrigin);
      features.push({
        type: 'Feature',
        properties: { type: 'grid-line' },
        geometry: {
          type: 'LineString',
          coordinates: [start, end],
        },
      });
    }

    // Horizontal lines (constant Y)
    for (let y = gridMinY; y <= gridMaxY; y += gridSpacing) {
      const start = metersToMapCoords(gridMinX, y, gpsOrigin);
      const end = metersToMapCoords(gridMaxX, y, gpsOrigin);
      features.push({
        type: 'Feature',
        properties: { type: 'grid-line' },
        geometry: {
          type: 'LineString',
          coordinates: [start, end],
        },
      });
    }

    return { type: 'FeatureCollection', features };
  }, [stations, gridSpacing, gpsOrigin]);

  // Add/update grid layer
  useEffect(() => {
    // No style while the WebGL context is lost: reading it throws (#576 NF-MAP-5).
    // A rebuilt style that has not loaded yet holds no grid and refuses the add
    // (RV18-MAP-1); styleEpoch runs this again once it has loaded.
    if (!map || !canWriteStyle(map)) return;

    const sourceId = 'grid-source';
    const layerId = 'grid-lines';

    const updateGrid = () => {
      const geojson = generateGridGeoJSON();
      const source = map.getSource(sourceId) as maplibregl.GeoJSONSource | undefined;
      if (source) {
        source.setData(geojson);
      }
    };

    if (showGrid && !layerAdded.current) {
      // Add source and layer
      const geojson = generateGridGeoJSON();

      if (!map.getSource(sourceId)) {
        map.addSource(sourceId, {
          type: 'geojson',
          data: geojson,
        });
      }

      if (!map.getLayer(layerId)) {
        // The grid belongs under every survey layer, ghosts and loop closures
        // included, which the first candidate gives; the last one only lands it
        // under the geometry, above ghosts and closures. No anchor means no
        // survey layer exists yet, and layers added after this one are appended
        // above it anyway.
        const firstSurveyLayer = BOTTOM_SURVEY_LAYERS.find((id) => map.getLayer(id));
        map.addLayer(
          {
            id: layerId,
            type: 'line',
            source: sourceId,
            paint: {
              'line-color': gridColor,
              'line-width': 1,
              'line-opacity': 0.8,
            },
          },
          firstSurveyLayer
        );
      }

      layerAdded.current = true;
    } else if (!showGrid) {
      // Remove layer and source. Not gated on layerAdded: a style restored after
      // a WebGL context loss can carry the grid back after it was turned off.
      if (map.getLayer(layerId)) {
        map.removeLayer(layerId);
      }
      if (map.getSource(sourceId)) {
        map.removeSource(sourceId);
      }
      layerAdded.current = false;
    } else if (showGrid && layerAdded.current) {
      // Update existing grid data
      updateGrid();
    }

    return () => {
      // Guard against map being destroyed (race condition on unmount)
      if (!hasLiveStyle(map)) {
        layerAdded.current = false;
        return;
      }

      try {
        if (map.getLayer(layerId)) {
          map.removeLayer(layerId);
        }
        if (map.getSource(sourceId)) {
          map.removeSource(sourceId);
        }
      } catch {
        // Map already destroyed during unmount, cleanup not needed
      }
      layerAdded.current = false;
    };
  }, [map, showGrid, generateGridGeoJSON, gridColor, styleEpoch]);

  // Update grid color when config changes
  useEffect(() => {
    if (!map || !showGrid || !hasLiveStyle(map)) return;

    const layerId = 'grid-lines';
    if (map.getLayer(layerId)) {
      map.setPaintProperty(layerId, 'line-color', gridColor);
    }
  }, [map, showGrid, gridColor, styleEpoch]);

  // Update grid when spacing changes
  useEffect(() => {
    if (!map || !showGrid || !hasLiveStyle(map)) return;

    const sourceId = 'grid-source';
    const source = map.getSource(sourceId) as maplibregl.GeoJSONSource | undefined;
    if (source) {
      source.setData(generateGridGeoJSON());
    }
  }, [map, showGrid, gridSpacing, generateGridGeoJSON, styleEpoch]);

  return null;
}
