/**
 * The single-select shot / station colouring mode and its mapping onto the
 * three mutually exclusive overlay flags. Its own module so `displaySelectors`
 * can derive the effective mode without importing the store (an import cycle:
 * the store reads `DISPLAY_DEFAULTS` at creation).
 */
import type { UIStore } from './uiStore';

/**
 * Shot/station coloring mode. The three overlay flags (showDepthGradient /
 * showDistanceGradient / showIssueSeverity) are mutually exclusive - all paint the
 * shots/stations layer - so this enum is the single-select view of them. `section`
 * = none of the overlays (the default per-section coloring). Read via
 * {@link colorModeOf}; set via the store's `setColorMode` (the one place that owns
 * the select-one mapping).
 */
export type ColorMode = 'section' | 'depth' | 'distance' | 'issues';

/** Derive the single-select coloring mode from the three mutually-exclusive
 *  overlay flags. Use as a zustand selector: `useUIStore(colorModeOf)`. */
export function colorModeOf(
  s: Pick<UIStore, 'showIssueSeverity' | 'showDistanceGradient' | 'showDepthGradient'>,
): ColorMode {
  return s.showIssueSeverity
    ? 'issues'
    : s.showDistanceGradient
      ? 'distance'
      : s.showDepthGradient
        ? 'depth'
        : 'section';
}

/**
 * The overlay flags (plus the distance overlay's cache) that represent a
 * {@link ColorMode} - the inverse of {@link colorModeOf}, and the ONE place that
 * maps a single-select mode back onto the three booleans.
 *
 * Every writer that can turn an overlay on goes through this, so "two overlays on
 * at once" is not representable after any entry point runs. That mattered:
 * `showDepthGradient` is persisted (a DisplayToggleKey) while
 * `showDistanceGradient` is runtime-only, so re-applying the saved display
 * settings while distance was on (project load / revision restore) wrote depth
 * back to true underneath it. `colorModeOf` reads that pair as 'distance', and
 * the old `if (!flag) toggle()` writers no-op'd on it - picking Depth did nothing
 * and the overlay was stuck in distance mode for good (#34, #22).
 *
 * `distanceGradientOrigin`/`distanceField` are the distance overlay's cache: left
 * alone when distance IS the mode, cleared with it otherwise, so re-enabling
 * starts fresh at the entrance (zero-datum) default.
 */
export type ColorOverlayFlags = Pick<
  UIStore,
  'showDepthGradient' | 'showDistanceGradient' | 'showIssueSeverity'
> &
  Partial<Pick<UIStore, 'distanceGradientOrigin' | 'distanceField'>>;

export function colorModeFlags(mode: ColorMode): ColorOverlayFlags {
  return {
    showDepthGradient: mode === 'depth',
    showDistanceGradient: mode === 'distance',
    showIssueSeverity: mode === 'issues',
    ...(mode === 'distance' ? {} : { distanceGradientOrigin: null, distanceField: null }),
  };
}
