// Drag-preview MapLibre layers (#mem drag fixes).
//
// The unified drag-preview source (`survey-drag-preview`) is drawn by a fixed
// stack of geometry layers, split out of SurveyLayer.ts to keep that file off
// its size ceiling and to hold the (verbose) layer specs in one testable place.
//
// Geometry layers (shots / closures / stations) render the captured MOVING
// coordinates: the source is rebuilt each frame via a single `setData`, so the
// tiler clips the real moved shape (no background wedges at large offsets).
//
// The text attached to moving features (station / shot labels, comments,
// section names) is NOT previewed - it is hidden from the stock label / comment
// / section-name layers for the drag duration (composed filters in SurveyLayer)
// and restored on commit-settle / Escape. Previewing it as well read as clutter.

import type * as maplibregl from 'maplibre-gl';
import type { ExpressionSpecification } from 'maplibre-gl';

export const DRAG_PREVIEW_SOURCE = 'survey-drag-preview';
const SHOTS_LAYER = 'survey-drag-preview-shots';
const CLOSURES_LAYER = 'survey-drag-preview-closures';
const STATIONS_LAYER = 'survey-drag-preview-stations';

const ALL_LAYER_IDS = [SHOTS_LAYER, CLOSURES_LAYER, STATIONS_LAYER];

/** Everything the layer specs need from SurveyLayer without importing it. */
export interface DragPreviewLayerCtx {
  /** shots + stations color (plain `['get','color']` or an analytical ramp). */
  colorExpr: ExpressionSpecification;
  shotWidth: number;
  stationRadius: number;
  selectHighlight: string;
}

/** Create the unified drag-preview source and its layers (idempotent). The
 *  layers filter one source by feature `kind`; z-order is lines (shots,
 *  closures) below circles (stations), matching the bulk stack. */
export function createDragPreviewLayers(map: maplibregl.Map, ctx: DragPreviewLayerCtx): void {
  if (map.getSource(DRAG_PREVIEW_SOURCE)) removeDragPreviewLayers(map);
  map.addSource(DRAG_PREVIEW_SOURCE, {
    type: 'geojson',
    data: { type: 'FeatureCollection', features: [] },
  });
  map.addLayer({
    id: SHOTS_LAYER,
    type: 'line',
    source: DRAG_PREVIEW_SOURCE,
    filter: ['==', ['get', 'kind'], 'shot'],
    paint: {
      'line-color': ctx.colorExpr,
      'line-width': ctx.shotWidth,
      'line-opacity': 0.85,
      // Viewport (screen-px) anchor so a synchronous per-pointermove translate
      // tracks the cursor 1:1 (see setDragPreviewTranslate + the ghost-follow
      // note in SurveyLayer). Starts at [0,0]; the re-baked geometry carries the
      // bulk of the offset, this only spans the residual to the live cursor.
      'line-translate-anchor': 'viewport',
    },
  });
  map.addLayer({
    id: CLOSURES_LAYER,
    type: 'line',
    source: DRAG_PREVIEW_SOURCE,
    filter: ['==', ['get', 'kind'], 'closure'],
    paint: {
      'line-color': ['get', 'color'],
      'line-width': 3,
      'line-opacity': 0.85,
      'line-dasharray': [6, 4],
      'line-translate-anchor': 'viewport',
    },
  });
  map.addLayer({
    id: STATIONS_LAYER,
    type: 'circle',
    source: DRAG_PREVIEW_SOURCE,
    filter: ['==', ['get', 'kind'], 'station'],
    paint: {
      'circle-color': ctx.colorExpr,
      'circle-radius': ctx.stationRadius,
      'circle-stroke-color': ctx.selectHighlight,
      'circle-stroke-width': 2.5,
      'circle-opacity': 0.85,
      'circle-translate-anchor': 'viewport',
    },
  });
}

/**
 * Apply a synchronous screen-space (viewport-anchored) translate to every
 * drag-preview layer at once, so all moving geometry follows the cursor in
 * lockstep with no worker round-trip (a bare `setPaintProperty`, applied on the
 * next render). Uniform across shots / closures / stations, so they cannot skew
 * apart. Idempotent + guarded for missing layers (mid-teardown / never created).
 */
export function setDragPreviewTranslate(map: maplibregl.Map, offset: [number, number]): void {
  for (const id of ALL_LAYER_IDS) {
    if (!map.getLayer(id)) continue;
    const prop = id === STATIONS_LAYER ? 'circle-translate' : 'line-translate';
    map.setPaintProperty(id, prop, offset);
  }
}

/** Remove the unified drag-preview source and layers (idempotent). */
export function removeDragPreviewLayers(map: maplibregl.Map): void {
  for (const id of ALL_LAYER_IDS) {
    if (map.getLayer(id)) map.removeLayer(id);
  }
  if (map.getSource(DRAG_PREVIEW_SOURCE)) map.removeSource(DRAG_PREVIEW_SOURCE);
}
