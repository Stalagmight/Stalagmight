/**
 * The stock MapLibre GeoJSON auxiliaries SurveyLayer keeps beside the custom
 * WebGL survey geometry (#576 D3 split): loop closures, the gas route highlight,
 * the merge-preview ghosts and their debug hitboxes, the snap highlight and the
 * interact overlay. Their ids, creation, teardown and config / theme paint live
 * here; SurveyLayer decides when each runs and pushes their data
 * (auxiliaryData.ts).
 */
import type { Map as MaplibreMap } from 'maplibre-gl';
import type { MapThemeColors } from '../../../utils/themeColors';

/** Source / layer ids of the auxiliaries, plus the stable z-order anchors. */
export const AUX_IDS = {
  sourceId: 'survey-data',
  // Stable ids kept only as z-order fallback anchors for the custom measure-path
  // layer's beforeId (the stock layers themselves are gone).
  stationsLayerId: 'survey-stations',
  loopClosuresLayerId: 'survey-loop-closures',
  ghostShotsLayerId: 'survey-ghost-shots',
  ghostStationsLayerId: 'survey-ghost-stations',
  ghostMergeLayerId: 'survey-ghost-merge',
  stationsHighlightLayerId: 'survey-stations-highlight',
  // Interact overlay (#mem): a tiny GeoJSON source holding ONLY the currently
  // hovered / selected / snapped shot(s)/station(s) as individual features.
  // With merged bulk features, per-item hover/selection can no longer use
  // per-feature feature-state; instead these few features are redrawn above
  // the bulk layers with the same resolved paint values, so pixels match.
  interactSourceId: 'survey-interact',
  interactShotsLayerId: 'survey-interact-shots',
  interactShotsMeasuredLayerId: 'survey-interact-shots-measured',
  interactStationsLayerId: 'survey-interact-stations',
  // NOTE: the former dedicated survey-shots-hitbox / survey-stations-hitbox
  // layers were removed (#mem budget). Each was a full unfiltered duplicate
  // vertex bucket of its bulk source (~75k line segments / ~90k points) built
  // solely to gate hit-testing. The candidate-section gate now queries the
  // VISIBLE shots/stations layers with a padded probe box that reproduces the
  // old hit tolerance exactly - see hitCandidateSections().
  // Gas route highlight (#233): the picked penetration route drawn over the survey - one line
  // layer, swum legs at full opacity and jump legs faded via a data-driven line-opacity (#243).
  gasRouteLayerId: 'survey-gas-route',

  ghostStationsHitboxLayerId: 'survey-ghost-stations-hitbox',
  ghostShotsHitboxLayerId: 'survey-ghost-shots-hitbox',
} as const;

/** Config / theme inputs the auxiliary layers are created with. */
export interface AuxiliaryLayerStyle {
  shotHitArea: number;
  stationHitArea: number;
  shotWidth: number;
  stationRadius: number;
  themeColors: MapThemeColors;
}

/** Add the auxiliaries' (empty) GeoJSON sources. */
export function addAuxiliarySources(map: MaplibreMap): void {
  // Custom-only renderer: survey geometry + labels + measured-path tint are
  // drawn by the custom WebGL layers (SurveyCustomController /
  // SurveyLabelController / MeasurePathController) straight from the SoA
  // SurveyData - there are no stock bulk vector-tile sources for shots /
  // stations / shot-gradients / labels anymore. Only the small GeoJSON
  // auxiliaries (loop closures, gas route, merge ghosts, interact overlay)
  // live here.

  // Loop closures line source
  map.addSource(AUX_IDS.sourceId + '-loop-closures', {
    type: 'geojson',
    data: {
      type: 'FeatureCollection',
      features: [],
    },
  });

  // Gas route highlight line source (#233): one LineString per route leg, `isJump` flagged.
  map.addSource(AUX_IDS.sourceId + '-gas-route', {
    type: 'geojson',
    data: {
      type: 'FeatureCollection',
      features: [],
    },
  });

  // Ghost section sources for merge preview
  map.addSource(AUX_IDS.sourceId + '-ghost-shots', {
    type: 'geojson',
    data: {
      type: 'FeatureCollection',
      features: [],
    },
  });

  map.addSource(AUX_IDS.sourceId + '-ghost-stations', {
    type: 'geojson',
    data: {
      type: 'FeatureCollection',
      features: [],
    },
  });

  map.addSource(AUX_IDS.sourceId + '-ghost-merge', {
    type: 'geojson',
    data: {
      type: 'FeatureCollection',
      features: [],
    },
  });

  // Interact overlay source (#mem): holds only the handful of currently
  // hovered / selected / snapped features as individual GeoJSON features
  // (see refreshInteractOverlay). Bounded by interaction state, not survey
  // size, so its retained cost is negligible.
  map.addSource(AUX_IDS.interactSourceId, {
    type: 'geojson',
    data: {
      type: 'FeatureCollection',
      features: [],
    },
  });
}

/** Add the auxiliary layers, bottom to top, on the sources above. */
export function addAuxiliaryLayers(map: MaplibreMap, style: AuxiliaryLayerStyle): void {
  // === HIT-TEST GATING ===
  // The dedicated survey-shots-hitbox / survey-stations-hitbox layers were
  // removed (#mem budget): each was a full unfiltered duplicate vertex bucket
  // of its bulk source. Hit-testing now queries the VISIBLE shots/stations
  // layers with a padded probe box (see hitCandidateSections). The ghost
  // hitbox layers below stay - they live on the tiny ghost sources and drive
  // the merge-preview debug visualization only.
  //
  // Layer order (bottom to top):
  // 1. survey-ghost-shots-hitbox (debug only)
  // 2. survey-ghost-stations-hitbox (debug only)
  // Then visual layers follow...

  // Ghost shot hitbox layer (for debug visualization only - NOT interactive)
  map.addLayer({
    id: AUX_IDS.ghostShotsHitboxLayerId,
    type: 'line',
    source: AUX_IDS.sourceId + '-ghost-shots',
    layout: {
      'line-cap': 'round',
    },
    paint: {
      'line-width': style.shotHitArea,
      'line-color': 'rgba(255, 255, 0, 0.3)', // Visible when debug enabled
      'line-opacity': 0, // Hidden by default, shown via updateGhostDebugVisualization
    },
  });

  // Ghost station hitbox layer (for debug visualization only - NOT interactive)
  map.addLayer({
    id: AUX_IDS.ghostStationsHitboxLayerId,
    type: 'circle',
    source: AUX_IDS.sourceId + '-ghost-stations',
    paint: {
      'circle-radius': style.stationHitArea,
      'circle-color': 'rgba(255, 255, 0, 0.3)', // Visible when debug enabled
      'circle-opacity': 0, // Hidden by default, shown via updateGhostDebugVisualization
      'circle-stroke-width': 0,
    },
  });

  // === VISUAL LAYERS ===

  // Ghost section shots layer (dashed, semi-transparent) - rendered first so it's behind everything
  // Uses config-based line width, updated via updateGhostVisualLayers()
  map.addLayer({
    id: AUX_IDS.ghostShotsLayerId,
    type: 'line',
    source: AUX_IDS.sourceId + '-ghost-shots',
    paint: {
      'line-color': ['get', 'color'],
      'line-width': style.shotWidth,
      'line-opacity': 0.5,
      'line-dasharray': [4, 2],
    },
  });

  // Ghost section stations layer (semi-transparent)
  // Uses config-based radius, updated via updateGhostVisualLayers()
  map.addLayer({
    id: AUX_IDS.ghostStationsLayerId,
    type: 'circle',
    source: AUX_IDS.sourceId + '-ghost-stations',
    paint: {
      'circle-radius': style.stationRadius,
      'circle-color': ['get', 'color'],
      'circle-stroke-width': 1,
      'circle-stroke-color': style.themeColors.strokeColor,
      'circle-opacity': 0.5,
    },
  });

  // Ghost merge station highlight layer
  map.addLayer({
    id: AUX_IDS.ghostMergeLayerId,
    type: 'circle',
    source: AUX_IDS.sourceId + '-ghost-merge',
    paint: {
      'circle-radius': 8,
      'circle-color': style.themeColors.selectHighlight,
      'circle-stroke-width': 2,
      'circle-stroke-color': style.themeColors.strokeColor,
      'circle-opacity': 0.8,
    },
  });

  // Loop closures layer (dashed lines with color based on error severity)
  map.addLayer({
    id: AUX_IDS.loopClosuresLayerId,
    type: 'line',
    source: AUX_IDS.sourceId + '-loop-closures',
    paint: {
      // Color based on error percentage: green (<1%), yellow (1-3%), orange (3-5%), red (>5%)
      'line-color': ['get', 'color'],
      'line-width': 3,
      'line-opacity': 0.8,
      'line-dasharray': [6, 4], // Dashed line pattern for loop closures
    },
  });

  // === OVERLAY LAYERS (above the custom-rendered survey geometry) ===
  // Custom-only renderer: survey shots / stations / shot-gradients / labels /
  // comments / measured-path tint are drawn by the custom WebGL layers (added
  // by their controllers), NOT stock MapLibre layers. What remains here are the
  // small auxiliary overlays that sit ABOVE the custom geometry:
  //   - snap-station highlight + interact hover/selection overlay (interact source),
  //   - gas-route highlight.
  // The custom geometry layer inserts itself before `survey-interact-shots`
  // (see SurveyCustomController.enable), so it lands above loop closures /
  // ghosts and below every overlay added below. The custom label layer draws on
  // top of everything; the custom measure-path layer inserts before
  // `survey-interact-shots-measured` (see syncCustomMeasurePath).

  // Station highlight layer for snapped/active stations (interact source).
  // Snapped stations are pushed to the interact overlay as individual
  // `kind: 'snap'` features (setSnappedStation / setHighlightedStations).
  map.addLayer({
    id: AUX_IDS.stationsHighlightLayerId,
    type: 'circle',
    source: AUX_IDS.interactSourceId,
    filter: ['==', ['get', 'kind'], 'snap'],
    paint: {
      'circle-radius': style.stationRadius + 3, // Snapped state: radius + 3
      'circle-color': style.themeColors.snapColor,
      'circle-stroke-width': 4,
      'circle-stroke-color': style.themeColors.snapColor,
      // Disable default 300ms paint transitions - snap indicator must appear/disappear instantly
      'circle-opacity-transition': { duration: 0, delay: 0 },
      'circle-stroke-opacity-transition': { duration: 0, delay: 0 },
    },
  });

  // Gas route highlight (#233): the picked route drawn over the survey. Jump legs are
  // faded rather than dashed (#243) - the same half-opacity treatment reference shots
  // get - and unlike `line-dasharray`, `line-opacity` IS data-driven, so one layer
  // covers both leg kinds.
  map.addLayer({
    id: AUX_IDS.gasRouteLayerId,
    type: 'line',
    source: AUX_IDS.sourceId + '-gas-route',
    paint: {
      'line-color': ['coalesce', ['get', 'segmentColor'], '#22d3ee'],
      'line-width': 4,
      'line-opacity': ['case', ['==', ['get', 'isJump'], true], 0.45, 0.9],
    },
  });

  // === INTERACT OVERLAY LAYERS (#mem) ===
  // Individual hover/selection highlights, drawn from the tiny interact source,
  // stacked bottom-to-top: interact-shots (hover/select shot), then the measured
  // hover tint, then interact-stations, then the snap highlight. They are added
  // with explicit beforeIds so this relative order holds and the custom geometry
  // layer (inserted before interact-shots) sits beneath all of them. Colors are
  // resolved per feature at overlay-build time or via the same ramp expressions
  // in overlay modes.
  map.addLayer({
    id: AUX_IDS.interactStationsLayerId,
    type: 'circle',
    source: AUX_IDS.interactSourceId,
    filter: ['==', ['get', 'kind'], 'station'],
    paint: {
      'circle-radius': ['get', 'radius'],
      'circle-color': ['get', 'color'],
      'circle-opacity': ['get', 'opacity'],
      'circle-stroke-width': ['get', 'strokeWidth'],
      'circle-stroke-color': ['get', 'strokeColor'],
      'circle-stroke-opacity': ['get', 'opacity'],
    },
  }, AUX_IDS.stationsHighlightLayerId);

  map.addLayer({
    id: AUX_IDS.interactShotsMeasuredLayerId,
    type: 'line',
    source: AUX_IDS.interactSourceId,
    filter: ['==', ['get', 'kind'], 'shotMeasured'],
    layout: {
      'line-cap': 'round',
      'line-join': 'round',
    },
    paint: {
      'line-color': ['get', 'color'],
      'line-width': ['get', 'width'],
      'line-opacity': ['get', 'opacity'],
    },
  }, AUX_IDS.interactStationsLayerId);

  map.addLayer({
    id: AUX_IDS.interactShotsLayerId,
    type: 'line',
    source: AUX_IDS.interactSourceId,
    filter: ['==', ['get', 'kind'], 'shot'],
    paint: {
      'line-color': ['get', 'color'],
      'line-width': ['get', 'width'],
      'line-opacity': ['get', 'opacity'],
    },
  }, AUX_IDS.interactShotsMeasuredLayerId);
}

// Update ghost hitbox layer sizes from config. The main shots/stations hit
// areas are no longer layer paint (the dedicated hitbox layers were removed,
// #mem budget); they are read live per query in hitCandidateSections via
// getShot/StationHitReach(), so config changes take effect with no layer edit.
export function updateGhostHitboxLayers(map: MaplibreMap, stationHitArea: number, shotHitArea: number): void {
  if (!map) return;

  if (map.getLayer(AUX_IDS.ghostStationsHitboxLayerId)) {
    map.setPaintProperty(AUX_IDS.ghostStationsHitboxLayerId, 'circle-radius', stationHitArea);
  }
  if (map.getLayer(AUX_IDS.ghostShotsHitboxLayerId)) {
    map.setPaintProperty(AUX_IDS.ghostShotsHitboxLayerId, 'line-width', shotHitArea);
  }
}

// Update ghost visual layer sizes from config (merge preview layers)
export function updateGhostVisualLayers(map: MaplibreMap, baseRadius: number, baseWidth: number): void {
  if (!map) return;

  // Update ghost visual layers to match config (guard layer existence)
  if (map.getLayer(AUX_IDS.ghostStationsLayerId)) {
    map.setPaintProperty(AUX_IDS.ghostStationsLayerId, 'circle-radius', baseRadius);
  }
  if (map.getLayer(AUX_IDS.ghostShotsLayerId)) {
    map.setPaintProperty(AUX_IDS.ghostShotsLayerId, 'line-width', baseWidth);
  }
}

// Update debug visualization based on config.
// The main shots/stations hitbox layers were removed (#mem budget), so the
// show_hitboxes debug toggle no longer paints a visible outline for the main
// hit areas (hit-testing is a padded query on the visible layers now). The
// ghost hitbox layers still honor it for merge-preview debugging.
export function updateGhostDebugVisualization(map: MaplibreMap, showHitboxes: boolean): void {
  if (!map) return;

  const debugColor = 'rgba(255, 0, 0, 0.3)'; // Semi-transparent red for debug
  const opaqueColor = '#000000'; // OPAQUE color for normal mode (opacity controls visibility)

  // Ghost hitbox layers - debug visualization only (NOT used for hit detection)
  // Keep at opacity 0 when debug off to avoid unnecessary rendering work
  if (map.getLayer(AUX_IDS.ghostStationsHitboxLayerId)) {
    map.setPaintProperty(
      AUX_IDS.ghostStationsHitboxLayerId,
      'circle-color',
      showHitboxes ? debugColor : opaqueColor
    );
    map.setPaintProperty(
      AUX_IDS.ghostStationsHitboxLayerId,
      'circle-opacity',
      showHitboxes ? 1 : 0 // 0 when off - not used for hit detection
    );
  }

  if (map.getLayer(AUX_IDS.ghostShotsHitboxLayerId)) {
    map.setPaintProperty(
      AUX_IDS.ghostShotsHitboxLayerId,
      'line-color',
      showHitboxes ? debugColor : opaqueColor
    );
    map.setPaintProperty(
      AUX_IDS.ghostShotsHitboxLayerId,
      'line-opacity',
      showHitboxes ? 1 : 0 // 0 when off - not used for hit detection
    );
  }
}

/** Update the kept auxiliary layers' constant theme colors. */
export function applyAuxiliaryThemeColors(map: MaplibreMap, colors: MapThemeColors): void {
  // Ghost station stroke
  if (map.getLayer(AUX_IDS.ghostStationsLayerId)) {
    map.setPaintProperty(AUX_IDS.ghostStationsLayerId, 'circle-stroke-color', colors.strokeColor);
  }
  // Ghost merge station
  if (map.getLayer(AUX_IDS.ghostMergeLayerId)) {
    map.setPaintProperty(AUX_IDS.ghostMergeLayerId, 'circle-color', colors.selectHighlight);
    map.setPaintProperty(AUX_IDS.ghostMergeLayerId, 'circle-stroke-color', colors.strokeColor);
  }
  // Station highlight (snap color)
  if (map.getLayer(AUX_IDS.stationsHighlightLayerId)) {
    map.setPaintProperty(AUX_IDS.stationsHighlightLayerId, 'circle-color', colors.snapColor);
    map.setPaintProperty(AUX_IDS.stationsHighlightLayerId, 'circle-stroke-color', colors.snapColor);
  }
}

/**
 * Remove every auxiliary layer, then the sources (a source can only go once no
 * layer uses it). The custom survey geometry / labels / measured-path layers are
 * their controllers' to remove.
 */
export function removeAuxiliaryLayers(map: MaplibreMap): void {
  if (map.getLayer(AUX_IDS.stationsHighlightLayerId)) {
    map.removeLayer(AUX_IDS.stationsHighlightLayerId);
  }
  if (map.getLayer(AUX_IDS.gasRouteLayerId)) {
    map.removeLayer(AUX_IDS.gasRouteLayerId);
  }
  // Interact overlay layers (#mem)
  if (map.getLayer(AUX_IDS.interactShotsLayerId)) {
    map.removeLayer(AUX_IDS.interactShotsLayerId);
  }
  if (map.getLayer(AUX_IDS.interactShotsMeasuredLayerId)) {
    map.removeLayer(AUX_IDS.interactShotsMeasuredLayerId);
  }
  if (map.getLayer(AUX_IDS.interactStationsLayerId)) {
    map.removeLayer(AUX_IDS.interactStationsLayerId);
  }
  if (map.getLayer(AUX_IDS.loopClosuresLayerId)) {
    map.removeLayer(AUX_IDS.loopClosuresLayerId);
  }
  // Remove ghost section layers
  if (map.getLayer(AUX_IDS.ghostShotsLayerId)) {
    map.removeLayer(AUX_IDS.ghostShotsLayerId);
  }
  if (map.getLayer(AUX_IDS.ghostStationsLayerId)) {
    map.removeLayer(AUX_IDS.ghostStationsLayerId);
  }
  if (map.getLayer(AUX_IDS.ghostMergeLayerId)) {
    map.removeLayer(AUX_IDS.ghostMergeLayerId);
  }
  // Remove ghost hitbox layers (the main hitbox layers were removed, #mem)
  if (map.getLayer(AUX_IDS.ghostStationsHitboxLayerId)) {
    map.removeLayer(AUX_IDS.ghostStationsHitboxLayerId);
  }
  if (map.getLayer(AUX_IDS.ghostShotsHitboxLayerId)) {
    map.removeLayer(AUX_IDS.ghostShotsHitboxLayerId);
  }

  // Remove all sources after layers are removed
  if (map.getSource(AUX_IDS.sourceId + '-loop-closures')) {
    map.removeSource(AUX_IDS.sourceId + '-loop-closures');
  }
  if (map.getSource(AUX_IDS.sourceId + '-gas-route')) {
    map.removeSource(AUX_IDS.sourceId + '-gas-route');
  }
  // Remove ghost section sources
  if (map.getSource(AUX_IDS.sourceId + '-ghost-shots')) {
    map.removeSource(AUX_IDS.sourceId + '-ghost-shots');
  }
  if (map.getSource(AUX_IDS.sourceId + '-ghost-stations')) {
    map.removeSource(AUX_IDS.sourceId + '-ghost-stations');
  }
  if (map.getSource(AUX_IDS.sourceId + '-ghost-merge')) {
    map.removeSource(AUX_IDS.sourceId + '-ghost-merge');
  }
  if (map.getSource(AUX_IDS.interactSourceId)) {
    map.removeSource(AUX_IDS.interactSourceId);
  }
}
