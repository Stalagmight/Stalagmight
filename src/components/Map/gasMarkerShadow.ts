/**
 * The soft dark halo under the Plan-mode gas map markers (the zero-datum star, the marked
 * point, the stage-drop triangle), drawn as SVG strokes instead of a CSS filter (#576
 * NF-W13FE-CSS-D1, #605).
 *
 * On WebKitGTK's GPU painter an element under a CSS filter repaints through a GPU promise
 * image, and a critical memory release that lands while one is in flight deadlocks the web
 * process. The markers used to carry a `0 0 1px rgba(0,0,0,0.6)` shadow filter: 60% black
 * blurred with a 1 px standard deviation, no offset (fitted to Chromium's paint of it: the
 * pixels next to an edge measured 0.196, 0.059, 0.004 alpha against 0.195, 0.050, 0.006 for
 * that Gaussian). Outside an opaque shape's edge that is a fringe whose alpha falls from 0.3
 * at the edge to under 1/255 past 2.5 px. Here it is drawn as ten nested strokes of the
 * shape's own path under the shape, each reaching 0.25 px further out, their opacities chosen
 * so the bands composite to the Gaussian's mean alpha over each quarter pixel (0.270, 0.213,
 * 0.160, 0.115, 0.079, 0.051, 0.032, 0.018, 0.010, 0.005, innermost first). The shape itself
 * is opaque and covers the strokes' inner halves. The strokes take no pointer events, as the
 * filtered shadow took none.
 */

/** [how far past the outline the stroke reaches, in CSS px; its opacity], outermost first. */
const BANDS: ReadonlyArray<readonly [number, number]> = [
  [2.5, 0.0054],
  [2.25, 0.0049],
  [2.0, 0.0083],
  [1.75, 0.0133],
  [1.5, 0.0202],
  [1.25, 0.029],
  [1.0, 0.0394],
  [0.75, 0.0509],
  [0.5, 0.0626],
  [0.25, 0.0733],
];

/** Round to 3 decimals so the markup stays short. */
const r3 = (v: number) => Math.round(v * 1000) / 1000;

/**
 * The halo strokes for a marker path, as SVG markup to put before the path itself.
 *
 * @param d the shape's path data (in the marker's 24-unit viewBox)
 * @param sizePx the rendered size of the 24-unit viewBox, in CSS px
 * @param outlineWidth the shape's own stroke width, in viewBox units (it widens the outline)
 */
export function markerHalo(d: string, sizePx: number, outlineWidth: number): string {
  const unitsPerPx = 24 / sizePx;
  return BANDS.map(
    ([reachPx, opacity]) =>
      `<path d="${d}" fill="none" stroke="#000000" stroke-opacity="${opacity}" ` +
      `stroke-width="${r3(outlineWidth + 2 * reachPx * unitsPerPx)}" stroke-linejoin="round" pointer-events="none"/>`,
  ).join('');
}
