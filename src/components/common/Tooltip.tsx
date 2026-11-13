import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from 'react';
import { createPortal } from 'react-dom';

interface TooltipProps {
  /** Content shown inside the floating tooltip. */
  content: ReactNode;
  /** Trigger element(s) the tooltip is anchored to. */
  children: ReactNode;
  /** Extra classes for the inline-flex wrapper around the trigger. */
  className?: string;
  /** Extra classes for the floating tooltip bubble. */
  tooltipClassName?: string;
  /** Test ID applied to the floating tooltip bubble. */
  tooltipTestId?: string;
  /**
   * Hold the pointer this long before opening. Defaults to 0 (open on enter),
   * which is what every pre-existing caller expects. A delay is for triggers
   * sitting somewhere the cursor merely passes over - a map overlay, say - where
   * opening on every transit would flicker.
   */
  openDelayMs?: number;
  /**
   * Keep the bubble up this long after the pointer leaves. Defaults to 0.
   * Bridges the gap between a small trigger and its bubble.
   */
  closeDelayMs?: number;
  /**
   * Force the bubble open (`true`) or closed (`false`), ignoring hover. Leave
   * undefined to let hover govern. Lets a caller own a pinned/click-to-open
   * state while this component keeps owning placement.
   */
  open?: boolean | undefined;
  /** Fires whenever hover would open or close the bubble. */
  onHoverChange?: (hovered: boolean) => void;
  /**
   * Where the bubble sits. 'top' (default): centred above the trigger, below
   * it when there is no room above. 'right': beside the anchor's right edge
   * (+8px), top-aligned with it and clamped to the viewport; on the anchor's
   * left when there is no room on the right, and as 'top' when neither side
   * fits. For a bubble that must not cover its neighbours (a row's details
   * card next to the row instead of over the rows above it).
   */
  placement?: 'top' | 'right';
  /** The box 'right' places against (default: the trigger), e.g. the whole
   *  row header around a small name. */
  anchorRef?: RefObject<HTMLElement | null>;
}

/** Gap between the anchor and the bubble, and the bubble's viewport margin. */
const GAP = 8;
const MARGIN = 8;

interface TooltipPosition {
  left: number;
  top: number;
  transform: string;
}

/** 'top' placement: centred above the trigger, clamped sideways, below it
 *  when there is no room above. */
function placeTop(trigger: DOMRect, bubble: DOMRect | null): TooltipPosition {
  let left = trigger.left + trigger.width / 2;
  let top = trigger.top - 10;
  let transform = 'translate(-50%, -100%)';
  if (bubble) {
    const halfWidth = bubble.width / 2;
    if (left - halfWidth < MARGIN) left = MARGIN + halfWidth;
    else if (left + halfWidth > window.innerWidth - MARGIN) left = window.innerWidth - MARGIN - halfWidth;
    // If there isn't enough room above, place below the trigger.
    if (top - bubble.height < MARGIN) {
      top = trigger.bottom + 10;
      transform = 'translate(-50%, 0)';
    }
  }
  return { left, top, transform };
}

/**
 * Where the bubble goes (viewport px, for `position: fixed`). Exported for
 * tests. `bubble` is null before the bubble has been measured.
 */
export function tooltipPosition(
  placement: 'top' | 'right',
  trigger: DOMRect,
  anchor: DOMRect,
  bubble: DOMRect | null,
): TooltipPosition {
  if (placement === 'top') return placeTop(trigger, bubble);
  const right = { left: anchor.right + GAP, top: anchor.top, transform: 'none' };
  if (!bubble) return right;
  let left = right.left;
  if (left + bubble.width > window.innerWidth - MARGIN) {
    left = anchor.left - GAP - bubble.width;
    if (left < MARGIN) return placeTop(trigger, bubble);
  }
  const top = Math.max(MARGIN, Math.min(anchor.top, window.innerHeight - MARGIN - bubble.height));
  return { left, top, transform: 'none' };
}

/**
 * Generic hover tooltip that anchors a themed bubble to arbitrary children.
 *
 * Rendering happens in a body portal (so it escapes overflow/stacking contexts)
 * and the position is clamped to the viewport, flipping below the trigger when
 * there isn't room above. This is the app-styled alternative to the native
 * `title=` attribute. The positioning logic mirrors {@link HelpTooltip}.
 */
export function Tooltip({
  content,
  children,
  className = '',
  tooltipClassName = '',
  tooltipTestId,
  openDelayMs = 0,
  closeDelayMs = 0,
  open,
  onHoverChange,
  placement = 'top',
  anchorRef,
}: TooltipProps) {
  const triggerRef = useRef<HTMLSpanElement>(null);
  const tooltipRef = useRef<HTMLSpanElement>(null);
  const [isHovered, setIsHovered] = useState(false);
  const hoverTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [position, setPosition] = useState<TooltipPosition>({
    left: 0,
    top: 0,
    transform: 'translate(-50%, -100%)',
  });

  // A controlled `open` wins outright; otherwise hover decides.
  const isOpen = open ?? isHovered;

  const scheduleHover = useCallback(
    (hovered: boolean) => {
      if (hoverTimerRef.current !== null) {
        clearTimeout(hoverTimerRef.current);
        hoverTimerRef.current = null;
      }
      const delay = hovered ? openDelayMs : closeDelayMs;
      const apply = () => {
        hoverTimerRef.current = null;
        setIsHovered(hovered);
        onHoverChange?.(hovered);
      };
      if (delay <= 0) {
        apply();
        return;
      }
      hoverTimerRef.current = setTimeout(apply, delay);
    },
    [openDelayMs, closeDelayMs, onHoverChange],
  );

  // A trigger unmounted mid-delay (the map layer toggling off under the cursor)
  // must not fire a state update afterwards.
  useEffect(
    () => () => {
      if (hoverTimerRef.current !== null) clearTimeout(hoverTimerRef.current);
    },
    [],
  );

  const updatePosition = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const triggerRect = trigger.getBoundingClientRect();
    const anchorRect = anchorRef?.current?.getBoundingClientRect() ?? triggerRect;
    const bubble = tooltipRef.current?.getBoundingClientRect() ?? null;
    setPosition(tooltipPosition(placement, triggerRect, anchorRect, bubble));
  }, [placement, anchorRef]);

  useLayoutEffect(() => {
    if (!isOpen) return;

    // Place it before the first paint (no frame at the viewport's corner),
    // then again once the bubble's content has laid out.
    updatePosition();
    const rafId = requestAnimationFrame(updatePosition);
    const reposition = () => updatePosition();

    window.addEventListener('resize', reposition);
    window.addEventListener('scroll', reposition, true);

    return () => {
      cancelAnimationFrame(rafId);
      window.removeEventListener('resize', reposition);
      window.removeEventListener('scroll', reposition, true);
    };
  }, [isOpen, updatePosition]);

  const tooltip =
    isOpen && typeof document !== 'undefined'
      ? createPortal(
          <span
            ref={tooltipRef}
            // `break-words`, not `break-all`: break a long unbreakable token
            // (the lock-holder email this was originally sized for) only when it
            // genuinely does not fit, rather than chopping ordinary prose
            // mid-word. Attribution read "the GIS Us / er Community" under
            // `break-all`, and source names have to be legible.
            className={`pointer-events-none fixed z-[10000] max-w-96 rounded border border-border bg-panel-90 px-3 py-2 text-sm leading-snug text-text-primary shadow-lg whitespace-pre-line break-words ${tooltipClassName}`}
            style={{
              left: `${position.left}px`,
              top: `${position.top}px`,
              transform: position.transform,
            }}
            role="tooltip"
            data-testid={tooltipTestId}
          >
            {content}
          </span>,
          document.body
        )
      : null;

  return (
    <span
      ref={triggerRef}
      className={`inline-flex min-w-0 ${className}`}
      onMouseEnter={() => scheduleHover(true)}
      onMouseLeave={() => scheduleHover(false)}
    >
      {children}
      {tooltip}
    </span>
  );
}

export default Tooltip;
