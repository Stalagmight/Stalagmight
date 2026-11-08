import { useLayoutEffect, useRef, useState, type MutableRefObject } from 'react';
import { ROW_HEIGHT } from '../common/SpreadsheetGrid';

/** Mirrors the private `HEADER_HEIGHT` in
 *  `src/components/common/SpreadsheetGrid/SpreadsheetGrid.tsx` (the grid does not
 *  export it). Kept as a named constant so the arithmetic below reads as
 *  "header + rows" rather than as a magic 33. */
const HEADER_HEIGHT = 33;

/** Never size a filled grid below this on a short window - the same floor
 *  `ShotTableEditor`'s `MIN_GRID_HEIGHT` uses. */
const MIN_FILL_HEIGHT = 120;

/**
 * Height for a CONTENT-SIZED plan grid: the header plus however many rows there
 * are, capped at `maxRows` (past which the grid scrolls internally).
 *
 * Still the FALLBACK both plan grids fill from: it is what they render at before
 * the first measurement, and the height they keep under jsdom, which has no
 * `ResizeObserver` for {@link usePlanGridFillHeight} to measure with. An empty
 * table still gets one row of body height so the header does not sit on a
 * zero-height void.
 */
export function planGridHeight(rowCount: number, maxRows: number): number {
  return HEADER_HEIGHT + Math.min(Math.max(rowCount, 1), maxRows) * ROW_HEIGHT;
}

/**
 * Height for a plan grid that FILLS its tab body (#403 follow-up): measure the
 * wrapper the grid sits in and hand the grid that many pixels.
 *
 * The grid's `height` has to be a NUMBER - an indefinite height silently
 * disables virtualization - so a `h-full` wrapper is not enough on its own; the
 * pane has to be measured, exactly as `ShotTableEditor` measures its expanded
 * box. Guarded for jsdom, which has no `ResizeObserver` (the convention in
 * `ProfileChart`/`Dock`): with none, the height stays at `fallbackHeight`, so
 * component tests keep the deterministic content-sized height they had while
 * the cylinders grid shared its tab with the DPV editor.
 *
 * Attach `ref` to the element whose CONTENT box is the grid's room: the box that
 * directly wraps the grid and has no padding of its own (the tab body itself for
 * the Cylinders grid, the `flex-1 min-h-0` box above the no-rig hint for the
 * Segments one) - never a padded ancestor, whose content box is smaller than the
 * room the grid actually gets.
 */
export function usePlanGridFillHeight(fallbackHeight: number): {
  ref: MutableRefObject<HTMLDivElement | null>;
  height: number;
} {
  const ref = useRef<HTMLDivElement | null>(null);
  // `null` until the first measurement, so the fallback keeps tracking the row
  // count until a real box exists (and forever, under jsdom).
  const [measured, setMeasured] = useState<number | null>(null);
  // LAYOUT effect, unlike `ShotTableEditor`'s otherwise-identical passive one: the
  // pane is already laid out by the time this runs, and measuring before the paint
  // means a tab switch never shows one frame of a row-count-sized grid before it
  // snaps to the pane.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const measure = () => {
      // A zero box is not a measurement: jsdom lays nothing out (a test that shims
      // `ResizeObserver` still gets the row-count fallback), and in the browser it
      // just means the pane is currently hidden - the last real height is the
      // better answer either way.
      if (el.clientHeight <= 0) return;
      const h = Math.max(MIN_FILL_HEIGHT, el.clientHeight);
      // Sub-pixel churn would re-render the grid on every observer tick.
      setMeasured((prev) => (prev === null || Math.abs(prev - h) > 1 ? h : prev));
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, []);
  return { ref, height: measured ?? fallbackHeight };
}
