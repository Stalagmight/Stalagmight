import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { Tooltip } from '../common/Tooltip';
import { useTileAttribution } from '../../hooks/useTileAttribution';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';

/**
 * Below this host width the source names cannot fit on the bottom edge, so the
 * overlay collapses to the `i` affordance. Esri allows exactly this: "Offer an
 * expandable UI when names don't fit on small screens."
 *
 * Budgeted against everything sharing that edge, measured at 10px:
 *   335  the Esri World Imagery list (~66 characters) plus the bar's padding
 *  + 95  right inset: the Data dock's tab box, measured from the map column
 *  + 110 the MapLibre ScaleControl at the other end, plus a gap
 *  = 540.
 *
 * Undershooting here does not merely crowd the bar - it slides the names under
 * the scale bar, which is the obstruction Esri's terms forbid.
 */
const COMPACT_BELOW_PX = 540;

/** Pointer dwell before the compact panel opens. */
const HOVER_OPEN_MS = 350;
/** Grace after the pointer leaves, so a wobble does not dismiss it. */
const HOVER_CLOSE_MS = 150;

/**
 * Themed surface, shared by the inline bar and the compact button.
 *
 * These are the app's own tokens rather than Esri's recommended fixed palette
 * (#323232 on white at 65%), so a dark theme gets a dark chip instead of a white
 * card punched into the map. Esri's colours are a recommendation, not a term;
 * what the terms require is that the names be legible and unobstructed, and a
 * panel-coloured chip carrying the app's own text colour is legible in whichever
 * theme is actually on screen.
 *
 * Deliberately the same tokens the Data dock's tab uses (`PanelOpenerTab`), because
 * the bar sits directly alongside it on the map's bottom edge and the two should
 * read as one row of chrome.
 */
const CHIP_SURFACE = 'bg-panel-90 text-text-secondary border-border';

/**
 * Font size. Below Esri's recommended 12px minimum, which is design guidance
 * rather than a term of use; the requirement it serves - legible source names -
 * still holds at this size against a themed backing. Attribution is a credit the
 * map has to carry, not something it should compete with.
 */
const CHIP_TEXT = 'text-[10px] leading-none';

/**
 * The first source named in an attribution string, for the collapsed label.
 *
 * Derived from the live value rather than hard-coded, so it follows the provider
 * the imagery actually came from. Esri's reads "Source: Esri, Vantor, Earthstar
 * Geographics, and the GIS User Community" - the collapsed chip names the first
 * of those and the panel carries the rest, which is the shape Esri's "expandable
 * UI when names don't fit" describes.
 *
 * Returns '' when nothing can be read, and the chip falls back to the bare `i`
 * rather than inventing a name.
 */
export function primarySourceName(attribution: string): string {
  return (
    attribution
      // Both spellings appear across ArcGIS services.
      .replace(/^\s*sources?\s*:\s*/i, '')
      .split(',')[0]
      ?.trim() ?? ''
  );
}

interface MapAttributionProps {
  /** Tile provider whose imagery is on screen. */
  provider?: string;
  /**
   * Whether imagery is currently drawn. Attribution is required exactly as long
   * as the imagery is visible, so this must track the layer, not the map.
   */
  visible: boolean;
  /**
   * `auto` shows the names inline when there is room and collapses below
   * {@link COMPACT_BELOW_PX}. `compact` forces the collapsed form - for small
   * surfaces like the Section Preview, where inline names would cover the very
   * thing being previewed.
   */
  variant?: 'auto' | 'compact';
  /** Positioning classes; the caller owns where on the map this sits. */
  className?: string;
  /** Positioning styles, for insets that come from a shared layout constant. */
  style?: CSSProperties;
  /**
   * Called with the attribution's element while it is on screen, and with `null`
   * when it goes, for a surface that keeps something else off it (#576 W71: the
   * project view keeps the import parse indicator clear of the source names).
   */
  elementRef?: (el: HTMLDivElement | null) => void;
}

/**
 * The on-map data attribution for satellite imagery.
 *
 * Esri splits its attribution requirements in two, and only one half belongs
 * here. The *source names* must be "directly on or at the bottom of the map
 * where it is always visible" - that is this component. The "Powered by Esri"
 * notice may live "in a window that is accessible from a menu or button", so it
 * is credited in the About dialog instead and deliberately does NOT appear here;
 * spending scarce map real estate on text the licence does not require it to
 * carry makes the part that IS required harder to read. See
 * https://developers.arcgis.com/documentation/esri-and-data-attribution/interactive-maps/
 */
export function MapAttribution({
  provider = 'esri',
  visible,
  variant = 'auto',
  className = '',
  style,
  elementRef,
}: MapAttributionProps) {
  const { t } = useTranslation();
  // Gated on `visible`: this component is mounted unconditionally by every
  // surface and hidden by the prop, so an ungated fetch contacted the imagery
  // provider even for a project opened with satellite off.
  const { text } = useTileAttribution(provider, visible);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const setWrapper = useCallback(
    (el: HTMLDivElement | null) => {
      wrapperRef.current = el;
      elementRef?.(el);
    },
    [elementRef],
  );
  const [narrow, setNarrow] = useState(false);
  const [pinned, setPinned] = useState(false);

  // Measure the map area this sits over, not the chip itself: the chip shrinks
  // to whatever it renders, so measuring it would make the decision circular.
  useLayoutEffect(() => {
    if (!visible || variant === 'compact') return;
    const host = wrapperRef.current?.parentElement;
    if (!host) return;

    const measure = () => setNarrow(host.clientWidth < COMPACT_BELOW_PX);
    measure();

    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(host);
    return () => observer.disconnect();
    // `text` is a dependency because the component renders null until it has
    // one. For a provider absent from the compiled-in fallback table that first
    // render has no ref to measure, so without re-running when the live value
    // lands the observer is never installed and the bar stays wide on a host
    // too narrow for it - the obstruction COMPACT_BELOW_PX exists to prevent.
  }, [visible, variant, text]);

  // Clicking away closes a pinned panel. Dropping the button's `onBlur` along
  // with its `onFocus` (which was breaking click-to-open) took the only
  // outside-dismiss path with it, leaving a pinned panel sitting over the map
  // until someone found Escape - in a component whose whole job is to keep the
  // map's source names UNobstructed.
  useDismissableLayer({
    open: pinned,
    onDismiss: () => setPinned(false),
    contains: [wrapperRef],
  });

  // Escape closes a pinned panel, matching every other dismissible layer.
  useEffect(() => {
    if (!pinned) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setPinned(false);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [pinned]);

  // Imagery gone means the obligation is gone. Also drop the pin, so it does not
  // spring back open the next time the layer returns.
  useEffect(() => {
    if (!visible) setPinned(false);
  }, [visible]);

  const togglePinned = useCallback(() => setPinned((v) => !v), []);

  if (!visible || !text) return null;

  const isCompact = variant === 'compact' || narrow;
  const primarySource = primarySourceName(text);

  if (!isCompact) {
    return (
      <div
        ref={setWrapper}
        // A bottom-flush bar, not a floating card: `min-h-5` with centred
        // content rather than a vertical padding, so the names sit on the bar's
        // centre line instead of riding an edge. The height is the only thing
        // setting the space above and below - keep it a `min-`, so a text-zoom
        // that outgrows it pushes the bar taller instead of clipping. Top-left is the only
        // rounded corner, because it is the only exposed one - the bottom meets
        // the map's edge and the right butts the Data dock's tab. `border-r` is
        // dropped for the same reason: the tab brings its own left border, and
        // two abutting borders read as a double rule.
        className={`pointer-events-none flex min-h-5 select-none items-center border-l border-t px-2 rounded-tl ${CHIP_SURFACE} ${CHIP_TEXT} ${className}`}
        style={style}
        data-testid="map-attribution"
      >
        <span data-testid="map-attribution-text">{text}</span>
      </div>
    );
  }

  return (
    <div ref={setWrapper} className={className} style={style} data-testid="map-attribution">
      <Tooltip
        content={<span data-testid="map-attribution-text">{text}</span>}
        openDelayMs={HOVER_OPEN_MS}
        closeDelayMs={HOVER_CLOSE_MS}
        // Pinning wins over hover, so a click (or a tap, which produces no
        // hover at all) keeps the names up until they are dismissed.
        open={pinned || undefined}
        tooltipTestId="map-attribution-panel"
      >
        <button
          type="button"
          // Click is the ONLY thing that pins, deliberately. Opening on focus as
          // well made the first click a no-op: a browser focuses a button on
          // mouse-down, so `onFocus` opened the panel and the click that
          // followed toggled it straight back shut - leaving the names
          // unreachable by tap, where there is no hover to fall back on. All
          // three inputs still reach them: hover dwells, tap clicks, and a
          // keyboard Enter/Space arrives here as a click on a <button>.
          onClick={togglePinned}
          aria-expanded={pinned}
          aria-label={t('map.attribution.expandAria')}
          title={text}
          // Stays a rounded pill rather than inheriting the bar's single rounded
          // corner: it is a control, and one square corner reads as clipped.
          className={`flex h-5 items-center gap-1 rounded-full border ${primarySource ? 'pl-2 pr-1' : 'w-5 justify-center'} ${CHIP_SURFACE} ${CHIP_TEXT}`}
          data-testid="map-attribution-toggle"
        >
          {/* Naming the primary source keeps the collapsed state an attribution
              rather than an unexplained button - a bare `i` credits nobody. The
              rest of the list is one hover or click away, in the panel. */}
          {primarySource && (
            <span data-testid="map-attribution-label">
              {t('map.attribution.collapsed', { source: primarySource })}
            </span>
          )}
          <span
            aria-hidden
            className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full border border-current font-semibold"
          >
            i
          </span>
        </button>
      </Tooltip>
    </div>
  );
}

export default MapAttribution;
