import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { PinIcon } from '../common/icons';
import type { PanelReveal } from './usePanelReveal';

export type PanelEdge = 'left' | 'right' | 'bottom';

interface PanelOpenerTabProps {
  reveal: PanelReveal;
  edge: PanelEdge;
  /** Short panel name shown on the closed tab (e.g. "Lines"). */
  label: string;
  /** Chevron pointing into the viewport (open hint), e.g. ChevronRightIcon. */
  openArrow: ReactNode;
  /** Chevron pointing toward the window edge (close), e.g. ChevronLeftIcon. */
  closeArrow: ReactNode;
  /** Panel id used for test ids (`panel-tab-${id}` etc.). */
  testId: string;
  labels: { open: string; pin: string; unpin: string; close: string };
  /** Show the open-state controls while the panel is visually present (open OR
   *  sliding out), and only fall back to the opener label/arrow once fully
   *  minified - avoids the tab flickering its contents mid-transition. Defaults
   *  to `reveal.isOpen` when omitted. */
  visuallyOpen?: boolean;
  /** While the panel is being resized, tint the tab border with the accent so
   *  the resize highlight visually wraps around the tab instead of under it. */
  resizing?: boolean;
  /** The opener (edge bar + this tab) is hovered as a unit - highlight the
   *  closed tab (lighter bg + brighter text) together with the bar, regardless
   *  of whether the pointer is over the bar or the tab. */
  hovered?: boolean;
}

/**
 * Presentational opener tab for a panel. Rendered as a child of the Dock's outer
 * wrapper (which positions it flush to the panel's inner edge and owns the hover
 * region), so this component only renders the state-appropriate control box:
 *  - closed       → label + open-arrow (click/hover opens)
 *  - hover-open   → pin icon (click pins)
 *  - pinned-open  → active pin (click un-pins) + close-arrow (click closes)
 */
export function PanelOpenerTab({
  reveal,
  edge,
  label,
  openArrow,
  closeArrow,
  testId,
  labels,
  resizing = false,
  visuallyOpen,
  hovered = false,
}: PanelOpenerTabProps) {
  const open = visuallyOpen ?? reveal.isOpen;
  // Closed opener highlights with the bar (driven by the shared hover state).
  const openerHi = hovered && !open;
  const vertical = edge === 'left' || edge === 'right';

  // WebKit (the Tauri webview) computes the intrinsic block-size of a vertical
  // `writing-mode` text run once and does NOT recompute it when the page is
  // zoomed (Ctrl+), so the rotated label overflowed the tab on zoom. Measure the
  // label in NORMAL (horizontal) flow - which is zoom-correct - and pin that as
  // the vertical label's length, recomputing on resize (zoom fires `resize`).
  const measureRef = useRef<HTMLSpanElement>(null);
  const [labelLen, setLabelLen] = useState<number | null>(null);
  useLayoutEffect(() => {
    if (!vertical || open) {
      setLabelLen(null);
      return undefined;
    }
    const update = () => {
      const el = measureRef.current;
      if (el) setLabelLen(Math.ceil(el.getBoundingClientRect().width) + 2);
    };
    update();
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, [vertical, open, label]);
  // Omit the border on the panel-facing edge and overlap it by 1px (negative
  // margin) so the tab reads as part of the panel - no seam between them.
  const edgeCls =
    edge === 'left'
      ? 'border-y border-r rounded-r -ml-px'
      : edge === 'right'
        ? 'border-y border-l rounded-l -mr-px'
        : 'border-x border-t rounded-t -mb-px';

  const box = [
    `flex items-center justify-center transition-colors ${resizing ? 'border-accent' : 'border-border'} ${openerHi ? 'bg-background-80 text-text-primary' : 'bg-panel text-text-secondary'} select-none`,
    // min- (not fixed w/h) so the box grows to fit the label when the webview
    // text-zooms (Ctrl+): the text scales but a fixed px/rem box would not, so
    // the label overflowed. At rest it's still the ~28px zoom-button size.
    vertical ? 'flex-col gap-1 min-w-7 py-2' : 'flex-row gap-1 min-h-7 px-2',
    edgeCls,
  ].join(' ');

  if (!open) {
    return (
      <div className={box}>
        <button
          type="button"
          className={`flex items-center justify-center gap-1.5 ${vertical ? 'flex-col' : 'flex-row'} transition-colors`}
          onClick={reveal.openClick}
          title={labels.open}
          aria-label={labels.open}
          data-testid={`panel-tab-${testId}`}
        >
          <span aria-hidden className="flex items-center justify-center">
            {openArrow}
          </span>
          {vertical && (
            // Hidden horizontal twin, measured to drive the vertical label's
            // length (see labelLen). Absolute so it never affects layout.
            <span
              ref={measureRef}
              aria-hidden
              className="text-[11px] font-medium tracking-wide whitespace-nowrap pointer-events-none invisible absolute"
            >
              {label}
            </span>
          )}
          <span
            className="text-[11px] font-medium tracking-wide whitespace-nowrap"
            style={
              vertical
                ? { writingMode: 'vertical-rl', height: labelLen ?? undefined }
                : undefined
            }
          >
            {label}
          </span>
        </button>
      </div>
    );
  }

  const pinned = reveal.isPinnedOpen;

  const pinButton = (
    <button
      type="button"
      className={`flex items-center justify-center transition-colors ${pinned ? 'text-accent' : 'hover:text-text-primary'}`}
      onClick={pinned ? reveal.unpin : reveal.pin}
      title={pinned ? labels.unpin : labels.pin}
      aria-label={pinned ? labels.unpin : labels.pin}
      aria-pressed={pinned}
      data-testid={`panel-pin-${testId}`}
    >
      <PinIcon size={16} />
    </button>
  );

  const closeButton = pinned ? (
    <button
      type="button"
      className="flex items-center justify-center transition-colors hover:text-text-primary"
      onClick={reveal.close}
      title={labels.close}
      aria-label={labels.close}
      data-testid={`panel-close-${testId}`}
    >
      <span aria-hidden className="flex items-center justify-center">
        {closeArrow}
      </span>
    </button>
  ) : null;

  // The bottom (Data) tab puts the arrow before the pin; side tabs keep pin first.
  return (
    <div className={box}>
      {edge === 'bottom' ? (
        <>
          {closeButton}
          {pinButton}
        </>
      ) : (
        <>
          {pinButton}
          {closeButton}
        </>
      )}
    </div>
  );
}

export default PanelOpenerTab;
