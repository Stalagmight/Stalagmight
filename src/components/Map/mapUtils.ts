import type * as maplibregl from 'maplibre-gl';
import { isStyleRestoring } from './surveyLayer/customRender/styleRestore';

/**
 * Check if map instance has a live style (not destroyed).
 * Uses internal property access since maplibre-gl doesn't expose this publicly.
 * After map.remove() is called, the internal style object is destroyed,
 * and any calls to getLayer/getSource/etc will throw.
 */
export function hasLiveStyle(map: maplibregl.Map | null): boolean {
  return !!(map && (map as unknown as { style?: unknown }).style);
}

/**
 * Whether a write to the stock style (`addSource`, `addLayer`, `setFilter`,
 * `setLayoutProperty`, `setPaintProperty`, ...) can land now: the style is alive,
 * it has loaded, and no rebuild is under way.
 *
 * {@link hasLiveStyle} alone is not enough. A WebGL context restore and a worker
 * restart put a NEW style in place that loads one frame later (#606, see
 * surveyLayer/customRender/styleRestore.ts). Until its `style.load` that style
 * is empty - `getLayer` and `getSource` find nothing, so add-if-missing code
 * tries to add - and MapLibre refuses every write with "Style is not done
 * loading." (#576 RV18-MAP-1). A write skipped here is not lost: the restore
 * replays the stock style state once that style has loaded (SurveyLayer's
 * replay - the closure exclusion of a carry picked up in either window included,
 * RV19b-M1 - and `useStyleEpoch` for the React effects).
 */
export function canWriteStyle(map: maplibregl.Map | null): boolean {
  if (!map || !hasLiveStyle(map)) return false;
  // MapLibre's own `Style._checkLoaded` flag; a stand-in without it counts as loaded.
  if ((map as unknown as { style: { _loaded?: boolean } }).style._loaded === false) return false;
  return !isStyleRestoring(map);
}

/**
 * Anchor candidates, bottom-most first, for an overlay that must draw beneath
 * the survey: pass the first one `getLayer` resolves as `beforeId`.
 *
 * `addAuxiliaryLayers` adds the ghost shot hitbox first, so it is the bottom of
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
