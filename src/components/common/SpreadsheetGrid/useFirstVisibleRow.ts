/**
 * The first row visible below the sticky header, for the `headerCorner` prop.
 *
 * Read straight from the scroller's `scroll` event and kept in the state of
 * the component that calls the hook (the header corner), so scrolling
 * re-renders only that component and only when the index changes - never the
 * grid once per scrolled pixel.
 */
import { useEffect, useState, type RefObject } from 'react';
import { ROW_HEIGHT } from './types';

/** Index of the first row whose top is at or below the header's bottom edge,
 *  for a scroller at `scrollTop` (rows start right under the header, one
 *  ROW_HEIGHT each). A third of a pixel of tolerance absorbs the fractional
 *  scrollTop of scaled displays. Clamped to the rows; -1 when there are none. */
export function firstVisibleRowAt(scrollTop: number, rowCount: number): number {
  if (rowCount <= 0) return -1;
  return Math.min(rowCount - 1, Math.max(0, Math.ceil(scrollTop / ROW_HEIGHT - 0.01)));
}

export function useFirstVisibleRow(scrollRef: RefObject<HTMLElement | null>, rowCount: number): number {
  const [first, setFirst] = useState(0);
  useEffect(() => {
    const el = scrollRef.current;
    const update = () => setFirst(firstVisibleRowAt(el?.scrollTop ?? 0, rowCount));
    update();
    if (!el) return undefined;
    el.addEventListener('scroll', update, { passive: true });
    return () => el.removeEventListener('scroll', update);
  }, [scrollRef, rowCount]);
  // Until the effect re-reads the scroller, keep a stale index in range.
  return rowCount <= 0 ? -1 : Math.min(Math.max(first, 0), rowCount - 1);
}
