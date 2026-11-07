import { useCallback, useRef } from 'react';
import { useUIStore } from '../store/uiStore';

/**
 * Keep the import parse indicator ("Reading <file>...") above an element near the
 * window's bottom edge that it must not cover (#576 W71).
 *
 * The indicator is centred on the window and already floats above the status bar
 * through the bar's toast clearance (`useToastBottomClearance`). Attach the returned
 * callback ref to anything else in that strip: while it is mounted, the distance from
 * the window's bottom edge to its top edge is registered under `owner`, and the
 * indicator keeps its usual gap above whichever registered edge is highest.
 *
 * Measured as a position, not a height, because such an element need not sit on the
 * window's edge: the map attribution rides the map's bottom edge, above the status
 * bar and the Data dock's bar. So the parent is observed too - the map area resizes
 * when the Data panel opens or the window changes, which moves the element without
 * resizing it.
 *
 * A callback ref, like `useToastBottomClearance`: the element can mount and unmount
 * on its own (the attribution shows only while imagery is drawn).
 */
export function useParseIndicatorClearance(owner: string): (el: HTMLElement | null) => void {
  const register = useUIStore((s) => s.registerParseIndicatorBottomOffset);
  const unregister = useUIStore((s) => s.unregisterParseIndicatorBottomOffset);
  const release = useRef<(() => void) | null>(null);
  return useCallback(
    (el: HTMLElement | null) => {
      release.current?.();
      release.current = null;
      if (!el) return;
      // CSS px, the unit of the indicator's `bottom`.
      const report = () => register(owner, Math.max(0, window.innerHeight - el.getBoundingClientRect().top));
      report();
      const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(report);
      observer?.observe(el);
      if (el.parentElement) observer?.observe(el.parentElement);
      release.current = () => {
        observer?.disconnect();
        unregister(owner);
      };
    },
    [owner, register, unregister],
  );
}
