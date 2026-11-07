import { useEffect, type RefObject } from 'react';
import { useLatest } from './useLatest';

interface UseDismissableLayerOptions {
  /** Whether the layer is currently open (listeners attach only while open). */
  open: boolean;
  /** Dismiss the layer (called on outside mousedown, external scroll, or resize). */
  onDismiss: () => void;
  /** Elements that count as "inside": a mousedown within any of them is NOT an
   *  outside click. Typically the trigger plus the portaled surface(s). */
  contains: Array<RefObject<HTMLElement | null>>;
  /** Subset whose own scrolling must NOT dismiss (the portaled, scrollable surface).
   *  Beyond those, only the page and the layer's own ancestors dismiss, so the
   *  portal's captured position can't go stale; an unrelated scroller elsewhere in
   *  the app is ignored. Defaults to `contains`. */
  scrollSafe?: Array<RefObject<HTMLElement | null>>;
  /** The region the layer's captured position is anchored to: a scroll ANYWHERE
   *  inside it dismisses.
   *
   *  Needed by layers that pin themselves to cursor coordinates and render as a
   *  `position:fixed` sibling of the thing they point at, because then `contains`
   *  holds only the menu, whose own ancestors are just body/html - the scroll that
   *  actually strands it (the grid the header cell lives in) is neither an
   *  ancestor of the menu nor the page. Pass the enclosing panel and that scroll
   *  is covered, while a scroll outside the panel still is not. */
  anchorScope?: RefObject<HTMLElement | null>;
  /**
   * Opt OUT of the cursor-pinned default below. Set this only for a layer that
   * is positioned relative to a trigger element the scroll guard can find, and
   * that should therefore survive a scroll of an unrelated scroller.
   */
  survivesUnrelatedScroll?: boolean;
}

/**
 * Shared "dismiss a portaled popover/menu on outside interaction" behavior:
 * close on a mousedown outside the layer, on a scroll that could move it (the portal uses a
 * captured `position:fixed` rect that scrolling would invalidate), and on resize.
 *
 * Extracted from Select and AppMenu, which each had their own near-identical copy
 * of these three effects. Positioning (flip/clamp) stays per-component since the two
 * differ (Select matches the trigger width; AppMenu caps height and adds a submenu).
 *
 * The latest `onDismiss` and ref arrays are read at event time via refs, so the
 * three listeners bind once per open - a fresh inline `onDismiss`/array each render
 * doesn't re-bind them.
 */
export function useDismissableLayer({
  open,
  onDismiss,
  contains,
  scrollSafe,
  anchorScope,
  survivesUnrelatedScroll = false,
}: UseDismissableLayerOptions): void {
  const onDismissRef = useLatest(onDismiss);
  const containsRef = useLatest(contains);
  const scrollSafeRef = useLatest(scrollSafe ?? contains);
  const anchorScopeRef = useLatest(anchorScope);
  const survivesRef = useLatest(survivesUnrelatedScroll);

  useEffect(() => {
    if (!open) return undefined;
    // A `Select` nested inside this layer (e.g. one of its menu rows) portals its
    // own dropdown to `document.body`, so it is never a DOM descendant of `contains`
    // even though it is visually and logically part of this layer's content.
    // Without this, picking an option in a nested Select fires this layer's outside-
    // mousedown check first (mousedown precedes click) and dismisses the HOST layer
    // before the Select's own onChange can even land - the host (and the Select
    // inside it) unmounts out from under the click (#462).
    //
    // Resolve the listbox back to the combobox that owns it (via `aria-controls`,
    // which `Select` sets to the listbox's id whenever it's open) and test THAT
    // element against `contains`, rather than treating any `role="listbox"` as
    // inside unconditionally - other, unrelated listboxes exist in the app (e.g.
    // SectionsPanel's own `role="listbox"` section list), and accepting all of them
    // would suppress a legitimate outside-click dismiss for every open layer.
    const isInside = (refs: Array<RefObject<HTMLElement | null>>, node: Node | null) => {
      if (node === null) return false;
      if (refs.some((r) => r.current?.contains(node))) return true;
      if (!(node instanceof Element)) return false;
      const listbox = node.closest('[role="listbox"]');
      if (listbox === null || !listbox.id) return false;
      const owner = document.querySelector(`[aria-controls="${listbox.id}"]`);
      return owner !== null && refs.some((r) => r.current?.contains(owner));
    };

    const onMouseDown = (e: MouseEvent) => {
      if (!isInside(containsRef.current, e.target as Node)) onDismissRef.current();
    };
    const onScroll = (e: Event) => {
      const target = e.target;
      if (target instanceof Node && isInside(scrollSafeRef.current, target)) return;
      // Only a scroll that can MOVE this layer should close it. These layers are
      // fixed-position portals pinned to coordinates captured when they opened,
      // so the scrolls that invalidate that position are the page itself and any
      // ANCESTOR of the layer (or of its trigger, which is what callers put in
      // `contains`). A scroll in a sibling subtree cannot shift it by a pixel.
      //
      // It used to dismiss on ANY scroll in the document, which made a map
      // right-click close its own menu: selecting the station scrolls the data
      // table's row and the sections panel's section into view, and that scroll
      // arrived while the menu was opening. The menu flashed and vanished, and
      // only a second right-click - by which point the row was already in view,
      // so nothing scrolled - would stay open.
      // Only an Element can be the "unrelated scroller" this guard is about; a
      // non-Element target is the page itself (window or document), which moves
      // everything. Tested by shape rather than `target === window` because
      // jsdom hands the listener a different Window object than the global.
      const movesLayer =
        !(target instanceof Element) ||
        containsRef.current.some((r) => {
          const el = r.current;
          return el !== null && el !== target && target.contains(el);
        }) ||
        // ...or the scroll happened inside the region the layer is anchored to.
        // The ancestor test above cannot see this case: a menu pinned to cursor
        // coordinates renders as a fixed sibling of the scroller, so the scroller
        // contains the ANCHOR but not the menu.
        anchorScopeRef.current?.current?.contains(target) === true ||
        // A cursor-pinned layer holds coordinates captured at click time and has
        // no trigger element to re-measure against, so ANY scroll can leave it
        // floating over different content while its actions still target what
        // was under the cursor - acting on the wrong row. Closing is the only
        // safe default; a layer anchored to a real trigger opts out.
        !survivesRef.current;
      if (!movesLayer) return;
      onDismissRef.current();
    };
    const onResize = () => onDismissRef.current();

    document.addEventListener('mousedown', onMouseDown);
    window.addEventListener('scroll', onScroll, { passive: true, capture: true });
    window.addEventListener('resize', onResize);
    return () => {
      document.removeEventListener('mousedown', onMouseDown);
      window.removeEventListener('scroll', onScroll, { capture: true });
      window.removeEventListener('resize', onResize);
    };
  }, [open, anchorScopeRef, containsRef, onDismissRef, scrollSafeRef, survivesRef]);
}
