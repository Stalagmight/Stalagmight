import { memo, useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { PanelOpenerTab, type PanelEdge } from './PanelOpenerTab';
import { useResizeHandle } from './useResizeHandle';
import type { PanelReveal } from './usePanelReveal';
import { PANEL_RESIZE_GRACE_MS, PANEL_SLIDE_MS } from './layoutConstants';
import { clamp } from '../../utils/clamp';

/** Width/height of the always-present closed-state opener bar (px). */
export const BAR = 12;

/**
 * Clamp an opener-tab inset so the tab box can never leave its container (#260).
 *
 * The tab anchors are FIXED insets from one container edge (`right: tabInset` for
 * the bottom Data tab, `top: tabTop` for the side tabs). Once the container is
 * smaller than the inset the tab's whole box lands outside it: the Data tab walked
 * off the left edge of a narrow map column and rendered on top of the Sections
 * panel, and a side tab drops below its bar when an open Data panel shortens it.
 *
 * `container === null` means "not measured yet" (first paint, or a zero-layout
 * test environment) and passes the requested inset through unchanged, so a
 * degenerate measurement can't yank the tab to 0.
 *
 * A MEASURED zero is not that case and is clamped like any other reading. The two
 * used to be conflated on `container > 0`, which opted out in exactly the
 * situation the clamp exists for: a side bar whose overlay computes to height 0
 * (`closedBottomInset` >= the container height) is a real measurement, and
 * passing `top: 79` through then put the tab 79px below a zero-height bar -
 * outside the container, which is the escape being prevented.
 */
export function clampTabInset(requested: number, container: number | null, tab: number): number {
  if (container === null || !Number.isFinite(container)) return requested;
  return clamp(requested, 0, Math.max(0, container - tab));
}

interface DockProps {
  reveal: PanelReveal;
  edge: PanelEdge;
  /** Controlled panel size (store width for left/right, height for bottom). */
  size: number;
  minSize: number;
  maxSize: number;
  onResize: (size: number) => void;
  onResizeEnd: (size: number) => void;
  /** Panel content (already wrapped with placement-dim if applicable). */
  children: ReactNode;
  label: string;
  openArrow: ReactNode;
  closeArrow: ReactNode;
  testId: string;
  tabLabels: { open: string; pin: string; unpin: string; close: string };
  /** Top offset (px) for the side tab so it aligns with the panel's header /
   *  content divider. Ignored for the bottom edge (the tab is bottom-anchored). */
  tabTop?: number;
  /** Right inset (px) for the bottom-edge tab, so it can be moved clear of the
   *  Details tab on the right. Ignored for left/right edges. */
  tabInset?: number;
  /** Reports the tab's measured viewport rect whenever it changes. Lets a
   *  neighbour sit flush against the tab. Deliberately the RECT and not a width:
   *  this overlay is portaled and carries its own left/right offsets, so a
   *  consumer positioned in a different frame cannot reconstruct the tab's edge
   *  from `tabInset` plus a width. */
  onTabBoundsChange?: (rect: DOMRect) => void;
  /** Side panels (left/right) only: while this panel is MINIMIZED (showing the
   *  opener bar), raise the bar's bottom by this many px so it doesn't run
   *  full-height alongside an open Data panel. The reflow spacer is unaffected,
   *  so the Data panel still sits between the two side panels. */
  closedBottomInset?: number;
  /** Data table only: render content as a fixed fullscreen overlay (no reparent). */
  fullscreen?: boolean;
  keepMounted?: boolean;
  /** Bottom edge only: portal the absolute overlay into this element instead of
   *  rendering it inline, so it positions relative to that ancestor (lets the
   *  Data panel span the full Main width past the center column's side insets).
   *  The reflow spacer still renders in place. */
  overlayContainer?: HTMLElement | null;
  /** Bottom edge only: left/right offsets (px) for the overlay in the portal
   *  container's frame - so the Data panel stops at open side panels but fills
   *  the gap left by minimized ones. Default 0 (full width). */
  overlayLeft?: number;
  overlayRight?: number;
  /** Notified when this Dock's resize-drag starts/ends, so a sibling can react
   *  (e.g. side bars tracking the Data panel's edge live during its resize). */
  onResizingChange?: (resizing: boolean) => void;
  /** Drop the transition on the edges that follow a sibling panel's size so they
   *  track 1:1 (no easing lag): a side bar's `bottom` follows the Data panel, and
   *  the bottom panel's `left`/`right` follow the side panels. Set while that
   *  sibling is being dragged; kept otherwise so open/close still animates. */
  suspendSiblingTransition?: boolean;
}

/**
 * One dockable panel - the SOLE mount of its content. Reflows the map when
 * pinned (via a flow spacer that only the map reacts to) or floats over the map
 * in hover mode; the panel element itself never moves or remounts across
 * pin/unpin (only the spacer + z-index change), so the transition is seamless.
 * The opener tab is a child anchored to the panel's inner edge (`*:100%`) so it
 * rides the edge with zero gap. See the panel-reveal redesign blueprint.
 */
function DockComponent({
  reveal,
  edge,
  size,
  minSize,
  maxSize,
  onResize,
  onResizeEnd,
  children,
  label,
  openArrow,
  closeArrow,
  testId,
  tabLabels,
  tabTop = 72,
  tabInset = 8,
  onTabBoundsChange,
  closedBottomInset = 0,
  fullscreen = false,
  keepMounted = false,
  overlayContainer,
  overlayLeft = 0,
  overlayRight = 0,
  onResizingChange,
  suspendSiblingTransition = false,
}: DockProps) {
  const vertical = edge === 'left' || edge === 'right';
  const clipRef = useRef<HTMLDivElement>(null);
  // Nullable ref type (not `useRef<HTMLDivElement>(null)`) because the callback
  // ref below assigns it - a RefObject initialized with null is read-only.
  const outerRef = useRef<HTMLDivElement | null>(null);
  const handleRef = useRef<HTMLDivElement>(null);
  // #260: the tab anchors below are FIXED insets from one container edge, so once
  // the container is smaller than the inset the tab's box lands outside it
  // entirely (the bottom Data tab rendered over the Sections panel; a side tab
  // drops below its bar when an open Data panel shortens it). Measure the
  // container and the tab so the inset can be clamped to what actually fits.
  const [outerEl, setOuterEl] = useState<HTMLDivElement | null>(null);
  const [tabEl, setTabEl] = useState<HTMLDivElement | null>(null);
  // Only ONE dimension of each box feeds the anchor: the bottom tab is inset from
  // its container's RIGHT edge (so: width), the side tabs from the TOP edge (so:
  // height). Track that single number rather than the whole box - see the
  // observer below for why the other dimension must not be in the loop at all.
  // `null` (not 0) is "not measured yet", so a measured zero clamps like any
  // other reading (see clampTabInset).
  const anchorAxis: 'width' | 'height' = edge === 'bottom' ? 'width' : 'height';
  const [outerExtent, setOuterExtent] = useState<number | null>(null);
  const [tabExtent, setTabExtent] = useState<number | null>(null);
  const setOuterNode = useCallback((node: HTMLDivElement | null) => {
    outerRef.current = node;
    setOuterEl(node);
  }, []);
  // "active" while hovering the handle or dragging it; both the panel's
  // inner-edge border and the opener tab border tint accent then, identically.
  const [resizeHover, setResizeHover] = useState(false);
  // Hovering anywhere over the closed opener (the edge bar OR its tab, which
  // share the outer wrapper) highlights both as one unit.
  const [openerHover, setOpenerHover] = useState(false);

  const { isResizing, handleProps } = useResizeHandle({
    direction: vertical ? 'horizontal' : 'vertical',
    handlePosition: edge === 'right' ? 'start' : 'end',
    minSize,
    maxSize,
    measureRef: clipRef,
    onResize,
    onResizeEnd,
    // After a hover-mode resize, onMouseLeave was suppressed during the drag, so
    // re-evaluate the close here. The release pointer often lands on the handle's
    // overhang (just outside the panel body) - still part of the Dock region - so
    // hit-test the whole outer wrapper (panel + handle + tab). If the pointer
    // ended off it, close on a *generous* grace so the user can move back over.
    onResizeSettled: ({ clientX, clientY }) => {
      // A fast resize released past the window edge (with pointer capture) yields
      // coords outside the viewport, where elementFromPoint returns null - which
      // would read as "not over region" and wrongly schedule a close of the panel
      // being resized. Clamp into the viewport so it hit-tests the nearest
      // in-viewport point (the panel/handle at the edge) instead.
      const x = clamp(clientX, 0, window.innerWidth - 1);
      const y = clamp(clientY, 0, window.innerHeight - 1);
      const el = document.elementFromPoint(x, y);
      // Keep the accent only if the release landed back on the handle (no
      // mouseenter re-fires on an in-place release); otherwise clear it.
      setResizeHover(!!el && !!handleRef.current && handleRef.current.contains(el));
      if (!reveal.isFloatingOpen) return;
      const overRegion = !!el && !!outerRef.current && outerRef.current.contains(el);
      if (!overRegion) reveal.scheduleClose(PANEL_RESIZE_GRACE_MS);
    },
  });

  const resizeActive = isResizing || resizeHover;

  // Let a sibling (the side bars) track this panel's edge live during its drag.
  // The cleanup clears the flag if this Dock unmounts mid-drag (project/view
  // switch, fullscreen toggle, section-focus collapse): otherwise the parent's
  // sibling-resizing flag would stick true and permanently suspend the sibling
  // panels' open/close easing for the rest of the session.
  // Measure the overlay wrapper and the tab box so the tab anchors can be clamped
  // into the container (#260). One observer for both; `border-box` so the tab's
  // border counts (its box is what must stay inside).
  useEffect(() => {
    if (typeof ResizeObserver === 'undefined') return undefined;
    // ONLY the anchor's own axis. This previously stored the whole box and kept the
    // previous object when NEITHER dimension had moved - which a resize drag makes
    // useless: it fires ~60x/s and the dragged dimension moves every tick, so the
    // "unchanged" test was never true and the Dock re-rendered at observer rate for
    // a number its anchor does not even read. One number per element also hands the
    // bail-out to React itself: setting state to an identical primitive re-renders
    // nothing, so no hand-written comparison is needed.
    const read = (el: Element) => el.getBoundingClientRect()[anchorAxis];
    const sync = () => {
      if (outerEl) setOuterExtent(read(outerEl));
      if (tabEl) {
        const rect = tabEl.getBoundingClientRect();
        setTabExtent(rect[anchorAxis]);
        // Surfaced so a neighbour can butt against the tab rather than guess at
        // its edge. `tabInset` pins the tab's far edge within THIS overlay, whose
        // own offsets a consumer in another frame does not know; and the tab's
        // size moves with its contents (a label when closed, pin + close icons
        // when open) and with webview text-zoom. Only the rect is knowable.
        onTabBoundsChange?.(rect);
      }
    };
    sync();
    const ro = new ResizeObserver(sync);
    if (outerEl) ro.observe(outerEl);
    if (tabEl) ro.observe(tabEl);
    return () => ro.disconnect();
  }, [outerEl, tabEl, anchorAxis, onTabBoundsChange]);

  useEffect(() => {
    onResizingChange?.(isResizing);
    return () => {
      if (isResizing) onResizingChange?.(false);
    };
  }, [isResizing, onResizingChange]);

  // Keep content mounted through the slide-out so closing animates (single
  // instance - never remounts on pin/unpin/hover). Visibility is single-sourced
  // from reveal.isOpen: while closing, `isClosing` keeps children painted until
  // the outer wrapper's width/height transition ends (onTransitionEnd below),
  // with a SLIDE_MS timer as a fallback so content always unmounts even when no
  // `transitionend` fires (disabled-transition theme / 0-duration). No lagging
  // mounted-state mirror. `mounted = open || isClosing`.
  const open = reveal.isOpen;
  const [isClosing, setIsClosing] = useState(false);
  const wasOpenRef = useRef(open);
  useEffect(() => {
    if (open) {
      // Re-opening (or opening) cancels any in-flight close.
      if (isClosing) setIsClosing(false);
    } else if (wasOpenRef.current) {
      // open → closed: start the slide-out; keep children mounted until the
      // clip box's width/height transition ends.
      setIsClosing(true);
    }
    wasOpenRef.current = open;
  }, [open, isClosing]);
  // Fallback unmount: if the wrapper has no width/height transition to emit
  // `transitionend` (e.g. the e-ink theme forces `transition: none !important`,
  // or reduced motion / 0-duration), clear `isClosing` after the slide window so
  // a closed panel's content can never stay mounted forever. The precise
  // onTransitionEnd path clears it earlier in the normal animated case, which
  // cancels this timer via cleanup.
  useEffect(() => {
    if (!isClosing) return;
    const t = window.setTimeout(() => setIsClosing(false), PANEL_SLIDE_MS + 50);
    return () => window.clearTimeout(t);
  }, [isClosing]);
  const mounted = keepMounted || open || isClosing;

  const sizeNow = open ? size : BAR;
  const spacerSize = reveal.isPinnedOpen ? size : BAR;
  // Inline transition (robust vs a Tailwind arbitrary value) - suspended while
  // dragging so the panel edge + spacer track the pointer 1:1.
  // Edges that follow a sibling's size: the bottom panel's left/right follow the
  // side panels; a side bar's bottom follows the Data panel. Drop their easing
  // while that sibling is dragged so they track 1:1 (no lag).
  const trackingEdges = edge === 'bottom' ? ['left', 'right'] : ['bottom'];
  const ease = `${PANEL_SLIDE_MS}ms ease-out`;
  const sizeTransition = isResizing
    ? 'none'
    : [
        `width ${ease}`,
        `height ${ease}`,
        ...(suspendSiblingTransition ? [] : trackingEdges.map((p) => `${p} ${ease}`)),
      ].join(', ');

  // ── Reflow spacer (flow): only this pushes the map. ──
  const spacer = (
    <div
      className="flex-shrink-0"
      style={{
        ...(vertical ? { width: spacerSize } : { height: spacerSize }),
        transition: sizeTransition,
      }}
      aria-hidden
    />
  );

  // ── Overlay wrapper (absolute, overflow-visible so the tab can escape). ──
  // While a side panel is minimized, raise the bar's bottom clear of an open
  // Data panel (the reflow spacer is untouched, so layout is otherwise the same).
  const barBottom = !open ? closedBottomInset : 0;
  const outerStyle: CSSProperties = {
    ...(edge === 'left' ? { top: 0, bottom: barBottom, left: 0, width: sizeNow } : {}),
    ...(edge === 'right' ? { top: 0, bottom: barBottom, right: 0, width: sizeNow } : {}),
    ...(edge === 'bottom' ? { left: overlayLeft, right: overlayRight, bottom: 0, height: sizeNow } : {}),
  };
  const borderCls = edge === 'left' ? 'border-r' : edge === 'right' ? 'border-l' : 'border-t';
  const innerStyle: CSSProperties = vertical
    ? { position: 'absolute', top: 0, bottom: 0, width: size, [edge === 'right' ? 'right' : 'left']: 0 }
    : { position: 'absolute', left: 0, right: 0, bottom: 0, height: size };

  // Tab anchored flush to the inner edge (rides the wrapper's edge for free).
  // Side tabs sit at `tabTop` so their top aligns with the panel's header divider.
  //
  // #260: the inset is clamped to what the container can actually hold, so the
  // tab can never leave it. `left/right: 100%` is deliberate (the tab overhangs
  // the inner edge and that is the design); it's the inset along the OTHER axis
  // that escapes when the container shrinks - `right: tabInset` walks the bottom
  // tab off the left edge and over the Sections panel once the map column is
  // narrower than the inset, and `top: tabTop` pushes a side tab below its bar
  // once an open Data panel has shortened it. Until the first measurement the
  // extent is null and the requested inset is used unclamped; a measured zero is
  // clamped like any other reading (see clampTabInset).
  const tabAnchor: CSSProperties = (() => {
    const inset = clampTabInset(edge === 'bottom' ? tabInset : tabTop, outerExtent, tabExtent ?? 0);
    if (edge === 'bottom') return { bottom: '100%', right: inset };
    return edge === 'left' ? { left: '100%', top: inset } : { right: '100%', top: inset };
  })();

  // Resize handle on the inner edge (only while docked).
  const handleClass =
    edge === 'bottom'
      ? 'resize-handle-v left-0 right-0 top-0 h-1 -mt-0.5 cursor-ns-resize'
      : edge === 'right'
        ? 'resize-handle-h left-0 top-0 bottom-0 w-1 -ml-0.5 cursor-ew-resize'
        : 'resize-handle-h right-0 top-0 bottom-0 w-1 -mr-0.5 cursor-ew-resize';

  const overlay = (
    <div
      ref={setOuterNode}
      className={`absolute pointer-events-auto ${fullscreen ? 'z-40' : reveal.isFloatingOpen ? 'z-30' : 'z-20'}`}
      style={{ ...outerStyle, transition: sizeTransition }}
      // The slide-out ends when THIS wrapper's own width/height transition
      // finishes - unmount the children then (single-sourced from reveal.isOpen;
      // the SLIDE_MS timer above is only a fallback). Guard on target + property
      // so a child's color/other transition bubbling up doesn't end it early.
      onTransitionEnd={(e) => {
        if (
          isClosing &&
          !open &&
          e.target === outerRef.current &&
          (e.propertyName === (vertical ? 'width' : 'height'))
        ) {
          setIsClosing(false);
        }
      }}
      onMouseEnter={() => {
        reveal.onRegionEnter();
        setOpenerHover(true);
      }}
      // Don't let a hover panel close mid-drag when the pointer leaves the region.
      onMouseLeave={() => {
        setOpenerHover(false);
        if (!isResizing) reveal.onRegionLeave();
      }}
      // Focus returning to an already-open floating panel (e.g. tabbing back in
      // within the close grace) cancels the pending close - mirrors the pointer
      // onRegionEnter. Guarded by isFloatingOpen so merely tab-focusing a CLOSED
      // panel's opener tab never auto-reveals it (only an explicit click does).
      onFocus={() => {
        if (reveal.isOpen) reveal.onRegionFocus?.();
      }}
      // Keyboard close: a hover panel opened via the keyboard (the opener tab is a
      // real button, so Enter/Space reveals it) gets no mouseleave to trigger the
      // grace close. When focus leaves the whole region - and the pointer isn't
      // over it - schedule the close so it can't stay floating open forever.
      onBlur={(e) => {
        const next = e.relatedTarget;
        const stayingInRegion = next instanceof Node && outerRef.current?.contains(next);
        if (!stayingInRegion) reveal.onRegionBlur?.();
        if (reveal.isFloatingOpen && !openerHover && !stayingInRegion) {
          reveal.onRegionLeave();
        }
      }}
    >
      {/* Clip box = the visible panel body; collapses to the opener bar when closed.
          The accessible opener is the real <button> in PanelOpenerTab; this bar is
          only a pointer convenience, so it is not a focusable/announced control. */}
      {/* Background must be a SINGLE utility: applying `bg-panel` plus a hover
          `bg-background-80` lets bg-panel win (same-layer conflict), so the bar
          never highlighted. Pick one (mirrors the tab's exclusive ternary) so the
          closed opener bar + tab highlight together. */}
      <div
        ref={clipRef}
        className={`absolute inset-0 overflow-hidden ${!open && openerHover ? 'bg-background-80' : 'bg-panel'} ${borderCls} ${resizeActive ? 'border-accent' : 'border-border'} transition-colors ${reveal.isFloatingOpen ? 'shadow-xl' : ''} ${!open ? 'cursor-pointer' : ''}`}
        onClick={!open ? reveal.openClick : undefined}
        data-testid={`${testId}-panel-bar`}
      >
        {mounted &&
          (fullscreen ? (
            <div
              className="fixed inset-x-0 bottom-0 z-40 bg-panel"
              style={{ top: 'var(--toolbar-h, 48px)' }}
            >
              {children}
            </div>
          ) : (
            <div style={innerStyle} hidden={!open && !isClosing} aria-hidden={!open && !isClosing}>{children}</div>
          ))}
      </div>

      {/* Resize handle on the inner edge whenever the panel is open (pinned or
          hover-floating), so hover panels can be resized too. Transparent hit
          area only - the visible highlight is the edge border + tab border tint
          (driven by resizeActive), so nothing draws under the tab. */}
      {reveal.isOpen && !fullscreen && (
        <div
          ref={handleRef}
          data-testid={`${testId}-resize-handle`}
          className={`absolute bg-transparent ${handleClass}`}
          style={{ touchAction: 'none', zIndex: 1 }}
          onMouseEnter={() => setResizeHover(true)}
          onMouseLeave={() => setResizeHover(false)}
          {...handleProps}
        />
      )}

      {!fullscreen && (
        <div ref={setTabEl} className="absolute z-[2]" style={tabAnchor}>
          <PanelOpenerTab
            reveal={reveal}
            edge={edge}
            label={label}
            openArrow={openArrow}
            closeArrow={closeArrow}
            testId={testId}
            labels={tabLabels}
            resizing={resizeActive}
            visuallyOpen={open || isClosing}
            hovered={openerHover}
          />
        </div>
      )}
    </div>
  );

  // Portal mode (overlayContainer prop passed, even if momentarily null): render
  // into the container once the ref resolves, never inline - an inline first
  // paint would position against the wrong ancestor and then remount the whole
  // overlay (incl. the DataTable) when it reparents. The parent captures the
  // container via a callback ref in the commit phase; React flushes that setState
  // synchronously before the browser paints, so the container is non-null on the
  // first paint: the overlay attaches with no missing-content frame and the
  // DataTable mounts exactly once (into the portal). Inline mode (prop omitted)
  // renders the overlay in place, as the side Docks do.
  const portalMode = overlayContainer !== undefined;
  const overlayNode = portalMode
    ? overlayContainer
      ? createPortal(overlay, overlayContainer)
      : null
    : overlay;

  return (
    <>
      {spacer}
      {overlayNode}
    </>
  );
}

// Memoized: during a panel resize the parent re-renders ~60×/s (the store width
// updates each tick), but only the dragged Dock's `size` (or a sibling-tracking
// inset/overlay) actually changes. With a stable `reveal` (memoized in
// usePanelReveal), hoisted arrow elements, and memoized tabLabels, the other Docks
// and their PanelOpenerTab subtrees skip reconciliation on unrelated ticks.
export const Dock = memo(DockComponent);

export default Dock;
