/**
 * Pure hit-probe geometry for the survey layer's candidate-line gate (#mem).
 *
 * The dedicated `survey-shots-hitbox` / `survey-stations-hitbox` layers were
 * removed to reclaim two full duplicate vertex buckets. Hit-testing now queries
 * the VISIBLE shots/stations layers, whose rendered mark is far thinner than the
 * old hitboxes, with a padded probe box that restores the exact old tolerance.
 *
 * This module isolates the tolerance-preservation arithmetic so it can be unit
 * tested without a live MapLibre map. See `SurveyLayer.hitCandidateLines`.
 *
 * Reach model (matches MapLibre's point-in-feature test):
 *   - a point hits a LINE within `line-width / 2` of its centerline,
 *   - a point hits a CIRCLE within its `radius`.
 * So the removed hitboxes gave reach = `hit_area / 2` (shot line of width
 * `hit_area`) and reach = `hit_area` (station circle of radius `hit_area`).
 * A box padded by `pad` around the probe reaches `pad + visibleHalf` px along
 * each axis; setting `pad = reach - visibleHalf` restores that axis reach
 * exactly. The square box reaches farther on the diagonals (a strictly larger
 * candidate set), which the SoA resolver trims back with exact math, so no
 * candidate is lost and none leaks past tolerance.
 */

export type XY = { x: number; y: number };
export type ProbePoint = XY | [number, number];
export type ProbeGeometry = ProbePoint | [ProbePoint, ProbePoint];
/** `[[minX, minY], [maxX, maxY]]` - a MapLibre `queryRenderedFeatures` box. */
export type ProbeBox = [[number, number], [number, number]];

/** Small guard against subpixel rasterization at the exact tolerance boundary
 *  (err on the side of a slightly larger box, never smaller). */
export const PROBE_EPSILON = 0.5;

export function toXY(p: ProbePoint): [number, number] {
  return Array.isArray(p) ? [p[0], p[1]] : [p.x, p.y];
}

/** A probe geometry is a box when it is a 2-tuple whose elements are themselves
 *  points (arrays or {x,y}), not raw numbers of an `[x, y]` point. */
export function isBoxGeometry(g: ProbeGeometry): g is [ProbePoint, ProbePoint] {
  return (
    Array.isArray(g) &&
    g.length === 2 &&
    typeof g[0] !== 'number' &&
    typeof g[1] !== 'number'
  );
}

/** Normalize a point/box probe into a bbox expanded by `pad` px on every side. */
export function padProbeBox(geometry: ProbeGeometry, pad: number): ProbeBox {
  let minX: number;
  let minY: number;
  let maxX: number;
  let maxY: number;
  if (isBoxGeometry(geometry)) {
    const a = toXY(geometry[0]);
    const b = toXY(geometry[1]);
    minX = Math.min(a[0], b[0]);
    maxX = Math.max(a[0], b[0]);
    minY = Math.min(a[1], b[1]);
    maxY = Math.max(a[1], b[1]);
  } else {
    const c = toXY(geometry);
    minX = maxX = c[0];
    minY = maxY = c[1];
  }
  return [
    [minX - pad, minY - pad],
    [maxX + pad, maxY + pad],
  ];
}

/**
 * Padding that makes a box query on the thin visible mark reproduce the old
 * hitbox `reach`. `visibleHalf` is the visible mark's half extent (line
 * width / 2 or circle radius). Result: `pad + visibleHalf === reach` along each
 * axis (before the epsilon), clamped to >= 0 so a visible mark already wider
 * than the old hitbox never shrinks the box.
 */
export function probePad(reach: number, visibleHalf: number, eps: number = PROBE_EPSILON): number {
  return Math.max(0, reach - visibleHalf) + eps;
}
