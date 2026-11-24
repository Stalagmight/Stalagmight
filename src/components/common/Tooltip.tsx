import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
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
  /** ARIA role of the wrapper around the trigger: `none` where the trigger is
   *  a menu's row, so the menu still owns its items directly. */
  role?: 'none';
  /**
   * Where the bubble opens: `above` the trigger (the default, flipping below
   * when there is no room), `below` it (flipping above), or at its `side`,
   * right of it and vertically centred (flipping left when there is no room):
   * for a menu row, whose neighbours above and below a bubble must not cover
   * (demo finding V-3).
   */
  placement?: TooltipPlacement;
  /** Draw the bubble on the opaque panel surface (`bg-panel`) instead of the
   *  translucent one, so what it covers never shows through its text (V-3). */
  opaque?: boolean;
}

export type TooltipPlacement = 'above' | 'below' | 'side';

/** Gap between the trigger and the bubble, px. */
const TOOLTIP_GAP_PX = 10;
/** The bubble keeps this far from the viewport's edges, px. */
const TOOLTIP_MARGIN_PX = 8;

interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

/**
 * Where a bubble of `bubble`'s size goes for a trigger at `trigger` in a
 * `viewport`-sized window (see {@link TooltipProps.placement}): the fixed
 * position of its anchor point and the transform that hangs the bubble off
 * it. `bubble` is null before the bubble has rendered once (no clamping yet).
 */
export function tooltipPosition(
  placement: TooltipPlacement,
  trigger: Rect,
  bubble: { width: number; height: number } | null,
  viewport: { width: number; height: number },
): { left: number; top: number; transform: string } {
  const margin = TOOLTIP_MARGIN_PX;
  const gap = TOOLTIP_GAP_PX;
  if (placement === 'side') {
    let left = trigger.right + gap;
    let top = trigger.top + trigger.height / 2;
    let transform = 'translate(0, -50%)';
    if (bubble) {
      const roomRight = viewport.width - margin - (trigger.right + gap);
      const roomLeft = trigger.left - gap - margin;
      if (bubble.width > roomRight && roomLeft > roomRight) {
        left = trigger.left - gap;
        transform = 'translate(-100%, -50%)';
      }
      const half = bubble.height / 2;
      top = Math.min(Math.max(top, margin + half), viewport.height - margin - half);
    }
    return { left, top, transform };
  }
  let left = trigger.left + trigger.width / 2;
  const above = { top: trigger.top - gap, transform: 'translate(-50%, -100%)' };
  const below = { top: trigger.bottom + gap, transform: 'translate(-50%, 0)' };
  let at = placement === 'below' ? below : above;
  if (bubble) {
    const halfWidth = bubble.width / 2;
    if (left - halfWidth < margin) {
      left = margin + halfWidth;
    } else if (left + halfWidth > viewport.width - margin) {
      left = viewport.width - margin - halfWidth;
    }
    if (placement === 'below') {
      // If there isn't enough room below, place above the trigger.
      if (below.top + bubble.height > viewport.height - margin) at = above;
    } else if (above.top - bubble.height < margin) {
      // If there isn't enough room above, place below the trigger.
      at = below;
    }
  }
  return { left, ...at };
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
  role,
  placement = 'above',
  opaque = false,
}: TooltipProps) {
  const triggerRef = useRef<HTMLSpanElement>(null);
  const tooltipRef = useRef<HTMLSpanElement>(null);
  const [isHovered, setIsHovered] = useState(false);
  const hoverTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [position, setPosition] = useState({
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
    const tooltipEl = tooltipRef.current;
    setPosition(
      tooltipPosition(
        placement,
        trigger.getBoundingClientRect(),
        tooltipEl ? tooltipEl.getBoundingClientRect() : null,
        { width: window.innerWidth, height: window.innerHeight },
      ),
    );
  }, [placement]);

  useLayoutEffect(() => {
    if (!isOpen) return;

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
            className={`pointer-events-none fixed z-[10000] max-w-96 rounded border border-border ${opaque ? 'bg-panel' : 'bg-panel-90'} px-3 py-2 text-sm leading-snug text-text-primary shadow-lg whitespace-pre-line break-words ${tooltipClassName}`}
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
      role={role}
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
