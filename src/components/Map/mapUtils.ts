import type * as maplibregl from 'maplibre-gl';

/**
 * Check if map instance has a live style (not destroyed).
 * Uses internal property access since maplibre-gl doesn't expose this publicly.
 * After map.remove() is called, the internal style object is destroyed,
 * and any calls to getLayer/getSource/etc will throw.
 */
export function hasLiveStyle(map: maplibregl.Map | null): boolean {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return !!(map && (map as any).style);
}

/**
 * Anchor candidates, bottom-most first, for an overlay that must draw beneath
 * the survey: pass the first one `getLayer` resolves as `beforeId`.
 *
 * `initializeLayers` adds the ghost shot hitbox first, so it is the bottom of
 * the survey stack; the rest are live fallbacks, not decoration, since an
 * unresolved anchor means MapLibre appends the overlay ON TOP of everything.
 * `survey-custom-render` (the WebGL geometry) is last because anchoring there
 * puts the overlay under the geometry but above the ghosts and loop closures.
 * Every id here must be one production code actually creates - the bug this
 * list replaced was a lone `survey-shots`, a stock layer the custom renderer
 * removed.
 */
export const BOTTOM_SURVEY_LAYERS = [
  'survey-ghost-shots-hitbox',
  'survey-ghost-shots',
  'survey-custom-render',
] as const;

/**
 * Anchor candidates for the satellite raster: it goes below the grid, and the
 * grid below the survey. `grid-lines` has to lead - the grid anchors on the
 * same bottom-most survey layer, and a later insertion before that same anchor
 * lands nearer to it, i.e. above the grid, which 0.7-opacity raster then hides.
 */
export const SATELLITE_BOTTOM_CANDIDATES = ['grid-lines', ...BOTTOM_SURVEY_LAYERS] as const;
