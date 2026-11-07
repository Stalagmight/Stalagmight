// Drag-preview geometry source (#mem drag fixes).
//
// A move drag (move_station / move_shot / move_segment) shows a live preview of
// the moving survey geometry. The preview is rendered from ONE dedicated GeoJSON
// source that is rebuilt every frame from a captured set of ORIGINAL coordinates
// plus the current screen-space offset. This module holds the pure, map-free
// logic so it can be unit-tested and to keep SurveyLayer.ts off its size ceiling.
//
// Why one source rebuilt per frame instead of the old `*-translate` temp layers:
//   - `line-translate` shifts rendered pixels but NOT tile clipping, so beyond the
//     vector-tile buffer the moved line was already clipped at cut time and per-tile
//     clip masks expose the background as triangular wedges. Emitting real moved
//     coordinates makes the tiler clip the MOVED geometry, so no wedges at any
//     distance (BUG A).
//   - Mixing translate (stations, instant GPU uniform) with setData (shots, one
//     frame later) skews the two apart. Putting ALL moving geometry through the
//     SAME setData makes everything move atomically, so nothing skews (BUG B).
//
// Coordinates are projected from the captured originals EVERY frame (never
// accumulated, never cached across frames) so the preview stays correct when the
// zoom changes mid-drag.

/** Minimal projection surface - satisfied by a MapLibre map, mockable in tests. */
export interface Projector {
  project(lngLat: [number, number]): { x: number; y: number };
  unproject(point: [number, number]): { lng: number; lat: number };
}

/**
 * Which vertices of a captured feature follow the drag offset:
 *   - `uniform` / `both`: every vertex moves (whole segment, or a shot whose
 *     both endpoints are dragged),
 *   - `from`: only the first vertex moves (shot whose start station is dragged),
 *   - `to`: only the last vertex moves (shot whose end station is dragged),
 *   - `none`: nothing moves (stationary feature carried for context).
 */
export type MovingEnd = 'from' | 'to' | 'both' | 'uniform' | 'none';

/** How the captured coordinates turn into a GeoJSON geometry. */
export type DragEmit = 'point' | 'line' | 'midpoint';

export interface DragPreviewFeature {
  /** `point` → Point at coords[0]; `line` → LineString of all coords;
   *  `midpoint` → Point at the midpoint of the (transformed) first and last coord
   *  (used for shot labels that sit at the middle of a stretching shot). */
  emit: DragEmit;
  /** ORIGINAL [lng, lat] vertices captured at drag start. */
  coords: Array<[number, number]>;
  movingEnd: MovingEnd;
  /** Passed through to the emitted GeoJSON feature unchanged (colors, widths,
   *  gradient props, the `kind` discriminator the layers filter on). */
  properties: Record<string, unknown>;
}

export interface DragPreviewSet {
  features: DragPreviewFeature[];
}

/** True when the vertex at `index` (of `count` total) moves for this `movingEnd`. */
export function movesVertex(movingEnd: MovingEnd, index: number, count: number): boolean {
  switch (movingEnd) {
    case 'uniform':
    case 'both':
      return true;
    case 'from':
      return index === 0;
    case 'to':
      return index === count - 1;
    case 'none':
      return false;
  }
}

/**
 * Move one captured coordinate by a screen-space [dx, dy]: project it at the
 * current camera, offset in pixels, unproject back to lng/lat. Projecting from
 * the ORIGINAL every frame (not accumulating) keeps it correct across mid-drag
 * zoom changes.
 */
export function offsetVertex(
  coord: [number, number],
  projector: Projector,
  dx: number,
  dy: number,
): [number, number] {
  const p = projector.project(coord);
  const np = projector.unproject([p.x + dx, p.y + dy]);
  return [np.lng, np.lat];
}

/** Transform a captured feature's coordinates, moving only the vertices its
 *  `movingEnd` selects and leaving the rest at their true position. */
export function transformFeatureCoords(
  feature: DragPreviewFeature,
  projector: Projector,
  dx: number,
  dy: number,
): Array<[number, number]> {
  const count = feature.coords.length;
  return feature.coords.map((c, i) =>
    movesVertex(feature.movingEnd, i, count) ? offsetVertex(c, projector, dx, dy) : c,
  );
}

/**
 * Offset an interact-overlay point (a station highlight) by the drag if it
 * belongs to the moving set; otherwise return it unchanged. Used so a
 * selected/hovered station highlight tracks the drag while stationary overlay
 * features (snap ring, highlighted stations) stay put (#C).
 */
export function offsetOverlayPoint(
  coord: [number, number],
  moving: boolean,
  projector: Projector,
  dx: number,
  dy: number,
): [number, number] {
  return moving ? offsetVertex(coord, projector, dx, dy) : coord;
}

/**
 * Offset an interact-overlay shot's endpoints per its `movingEnd` (undefined /
 * `none` → stationary, returned unchanged). A one-endpoint shot stretches
 * exactly like the preview does.
 */
export function offsetOverlayShot(
  from: [number, number],
  to: [number, number],
  movingEnd: MovingEnd | undefined,
  projector: Projector,
  dx: number,
  dy: number,
): { from: [number, number]; to: [number, number] } {
  if (!movingEnd || movingEnd === 'none') return { from, to };
  return {
    from: movingEnd === 'to' ? from : offsetVertex(from, projector, dx, dy),
    to: movingEnd === 'from' ? to : offsetVertex(to, projector, dx, dy),
  };
}

/**
 * Build the full FeatureCollection for the drag preview source from the captured
 * set and the current offset. One call per frame; the result is handed to a
 * single `setData`, so every moving feature updates in lockstep.
 */
export function buildDragPreviewFeatureCollection(
  set: DragPreviewSet,
  projector: Projector,
  dx: number,
  dy: number,
): GeoJSON.FeatureCollection {
  const features: GeoJSON.Feature[] = [];
  for (const f of set.features) {
    const moved = transformFeatureCoords(f, projector, dx, dy);
    if (f.emit === 'point') {
      features.push({
        type: 'Feature',
        geometry: { type: 'Point', coordinates: moved[0]! },
        properties: f.properties,
      });
    } else if (f.emit === 'line') {
      features.push({
        type: 'Feature',
        geometry: { type: 'LineString', coordinates: moved },
        properties: f.properties,
      });
    } else {
      const a = moved[0]!;
      const b = moved[moved.length - 1]!;
      features.push({
        type: 'Feature',
        geometry: { type: 'Point', coordinates: [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2] },
        properties: f.properties,
      });
    }
  }
  return { type: 'FeatureCollection', features };
}
