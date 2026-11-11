import { useLayoutEffect, useState, type CSSProperties, type RefObject } from 'react';

/** Floor for the height cap so a degenerate gap still shows a scrollable sliver
 *  rather than a zero-height (invisible) menu. */
const MIN_MENU_HEIGHT = 64;

/** Breathing room kept between the menu and a boundary (the toolbar's bottom /
 *  the status bar's top, or the viewport edges), so a capped menu doesn't sit
 *  flush against the chrome. */
const EDGE_MARGIN = 8;

interface AnchoredMenuOptions {
  /** Horizontal anchoring relative to the trigger. */
  align: 'center' | 'right';
  /** Gap (px) between the trigger and the menu, and the left/right viewport margin.
   *  The top/bottom viewport margin is the fixed {@link EDGE_MARGIN}, not `gap`. */
  gap: number;
  /** Menu width (px). The hook uses this for the center/right alignment + left-clamp
   *  math AND sets it as the element's width (returned in the style), so width has a
   *  single source of truth - do NOT also put a `w-*` class on the menu. */
  width: number;
  /** Optional absolute cap (px) on the menu's height, applied on top of the
   *  per-side free-space cap - restores the old fixed `max-h-*` so a long list
   *  stays compact (and scrolls) on a tall window instead of growing to fill the
   *  whole free side. Omit to bound only by the available side space. */
  maxHeightCap?: number;
  /** Optional signal to re-run the placement measurement while the menu stays
   *  open (e.g. a string built from async content, so a loading -> N-row swap
   *  triggers a remeasure). Placement always recomputes on open; pass a value
   *  here ONLY when the menu's content can resize after that. */
  remeasureKey?: string | number;
  /** The element that scrolls, when it is not the menu itself (a fixed header
   *  above an `overflow-auto` list). The menu's own `scrollHeight` cannot see
   *  what that inner list hides under the previous cap, so a remeasure after
   *  the content grows (a search cleared, a folder opened) would keep the old,
   *  smaller height. Omit when the menu element is the scroller. */
  scrollRef?: RefObject<HTMLElement | null>;
}

interface Placement {
  top: number;
  left: number;
  maxHeight: number;
}

/**
 * Position a trigger-anchored dropdown with `position: fixed` so it escapes any
 * ancestor `overflow: hidden` (e.g. the Dock's clip box) - unlike `position:
 * absolute`, which gets clipped when the menu flips upward and makes its items
 * unclickable (clicks fall through to whatever is painted behind the clip).
 *
 * Mirrors the measure-then-place pattern in {@link Select} and the data-table
 * right-click header menu: a hidden first pass lets a `useLayoutEffect` measure
 * the menu's natural height before paint, so it flips to whichever side has more
 * room, clamps within the viewport, and caps `maxHeight` to the chosen side's
 * free space (a short window then scrolls the list instead of overflowing
 * off-screen). Dismissal/scroll/resize handling stays with the caller via
 * {@link useDismissableLayer}.
 *
 * Placement is computed on open, and again whenever {@link AnchoredMenuOptions.remeasureKey}
 * changes. Pass `remeasureKey` when the menu's content height can change while
 * `open` (e.g. a loading-row → N-row async swap) so it re-places instead of
 * sitting at a stale `top`/`maxHeight`; omit it when content is stable on open.
 *
 * Returns a `fixed` CSS style for the menu element; render the menu while `open`
 * and spread the returned object onto its `style`.
 */
export function useAnchoredFixedMenu(
  open: boolean,
  triggerRef: RefObject<HTMLElement | null>,
  menuRef: RefObject<HTMLElement | null>,
  { align, gap, width, maxHeightCap, remeasureKey, scrollRef }: AnchoredMenuOptions
): CSSProperties {
  const [placement, setPlacement] = useState<Placement | null>(null);

  useLayoutEffect(() => {
    if (!open) {
      setPlacement(null);
      return;
    }
    const trigger = triggerRef.current;
    const menu = menuRef.current;
    if (!trigger || !menu) return;

    const rect = trigger.getBoundingClientRect();
    // scrollHeight reports the element's full content height even when it is
    // currently max-height-clipped (+ overflow-auto) from the PRIOR placement, so a
    // remeasure after a loading -> N-row content swap still sees the true natural
    // height instead of reading back the stale cap via getBoundingClientRect alone.
    // An inner scroller (`scrollRef`) is clipped by the menu's cap without the menu
    // itself overflowing, so add back what it hides.
    const inner = scrollRef?.current;
    const innerHidden = inner ? Math.max(inner.scrollHeight - inner.clientHeight, 0) : 0;
    const naturalHeight = Math.max(menu.getBoundingClientRect().height + innerHidden, menu.scrollHeight);

    // Keep the menu clear of the app chrome that would otherwise clip/cover it:
    // the opaque top toolbar when flipping up, the bottom status bar when flipping
    // down. `--toolbar-h` / `--statusbar-h` are their measured heights, set on the
    // SingleSystemView root and inherited here; absent → 0. EDGE_MARGIN keeps a
    // capped menu from sitting flush against the bar.
    const cs = getComputedStyle(trigger);
    const topBound = (parseFloat(cs.getPropertyValue('--toolbar-h')) || 0) + EDGE_MARGIN;
    const bottomBound =
      window.innerHeight - (parseFloat(cs.getPropertyValue('--statusbar-h')) || 0) - EDGE_MARGIN;

    // Flip toward the side with more room (matches the prior on-open heuristic).
    const spaceBelow = bottomBound - rect.bottom - gap;
    const spaceAbove = rect.top - topBound - gap;
    const up = spaceAbove > spaceBelow;
    const sideSpace = Math.max(up ? spaceAbove : spaceBelow, 0);
    const cappedNatural = Math.min(naturalHeight, maxHeightCap ?? Infinity);
    const maxHeight = Math.max(Math.min(cappedNatural, sideSpace), MIN_MENU_HEIGHT);
    // Up: clamp the top to `topBound`. Down: clamp the BOTTOM to `bottomBound`
    // (so the MIN_MENU_HEIGHT floor in a tiny window can't push the menu down
    // past the status bar); `Math.max(topBound, …)` keeps it on-screen if even
    // that overflows upward.
    const top = up
      ? Math.max(topBound, rect.top - maxHeight - gap)
      : Math.max(topBound, Math.min(rect.bottom + gap, bottomBound - maxHeight));

    const rawLeft = align === 'center' ? rect.left + rect.width / 2 - width / 2 : rect.right - width;
    // Clamp the LOWER bound last so a menu wider than the viewport pins to the left
    // edge (gap) rather than inverting off-screen.
    const left = Math.max(gap, Math.min(rawLeft, window.innerWidth - width - gap));

    setPlacement({ top, left, maxHeight });
  }, [open, triggerRef, menuRef, align, gap, width, maxHeightCap, remeasureKey, scrollRef]);

  if (!placement) {
    // Hidden first pass: render at natural size, out of the way, so the layout
    // effect can measure height before we place and reveal it (no flicker).
    return { position: 'fixed', top: 0, left: 0, width, zIndex: 50, visibility: 'hidden', pointerEvents: 'none' };
  }
  return {
    position: 'fixed',
    top: placement.top,
    left: placement.left,
    width,
    maxHeight: placement.maxHeight,
    zIndex: 50,
    visibility: 'visible',
  };
}
