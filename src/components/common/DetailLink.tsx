/**
 * DetailLink - Reusable hyperlink component for navigating between survey elements.
 *
 * Features:
 * - Button element styled as link (actions that change state, not navigation)
 * - Accent color with underline styling
 * - Focus ring for keyboard accessibility
 * - Warning indicator for missing targets
 * - Generates stable test IDs based on element type and position
 * - ARIA labels for accessibility
 * - Hover/focus highlighting: optionally highlights corresponding map element on mouse
 *   hover or keyboard focus via onHoverStart/onHoverEnd callbacks. Mouse leave only
 *   clears highlight if the element is not keyboard-focused (prevents highlight loss
 *   when user moves mouse away but keeps element focused).
 */

import { useCallback, useRef, type CSSProperties } from 'react';
import { useUIStore } from '../../store/uiStore';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { EntitySymbol } from './toolIcons';

export type DetailLinkType = 'station' | 'shot' | 'line';

/**
 * Target identifier for station or shot links.
 * Both lineId and position are required.
 */
export interface StationOrShotTarget {
  lineId: number;
  /** Position within line: 0-indexed for stations, 1-indexed for shots */
  position: number;
}

/**
 * Target identifier for line links.
 * Only lineId is required.
 */
export interface LineTarget {
  lineId: number;
}

/**
 * Union type for all target types.
 * Use StationOrShotTarget for stations/shots, LineTarget for lines.
 */
export type DetailLinkTarget = StationOrShotTarget | LineTarget;

/**
 * Type guard to check if target has position (station or shot target).
 */
export function hasPosition(target: DetailLinkTarget): target is StationOrShotTarget {
  return 'position' in target && typeof target.position === 'number';
}

export interface DetailLinkProps {
  /** Type of element this link navigates to */
  type: DetailLinkType;
  /** Target identifier (lineId required, position required for stations/shots) */
  target: DetailLinkTarget;
  /** Text to display in the link */
  displayText: string;
  /** Callback when link is clicked */
  onSelect: () => void;
  /** Whether the target element is missing/invalid */
  isMissing?: boolean;
  /** Callback to announce selection change for screen readers */
  onAnnounce?: (message: string) => void;
  /** Optional explicit test ID (overrides auto-generated) */
  testId?: string;
  /** Additional CSS classes */
  className?: string;
  /** Callback when hover/focus starts (for map highlighting). Ignored if isMissing. */
  onHoverStart?: () => void;
  /** Callback when hover/focus ends (for map highlighting). */
  onHoverEnd?: () => void;
  /**
   * Short primary label for the Full-mode entity pill, shown before the dim id
   * suffix. Used for lines (the bare name, e.g. "Main Line"); station/shot
   * links derive their primary from the target position automatically.
   */
  pillLabel?: string;
}

/**
 * Generate a stable test ID based on element type and target.
 *
 * Formats:
 * - Station: "station-{lineId}-{position}-link"
 * - Shot: "shot-{lineId}-{position}-link"
 * - Line: "line-{lineId}-link"
 *
 * @throws Error if station/shot type is used without position
 */
export function generateTestId(type: DetailLinkType, target: DetailLinkTarget): string {
  if (type === 'line') {
    return `line-${target.lineId}-link`;
  }
  // Station and shot require position
  if (!hasPosition(target)) {
    throw new Error(`${type} links require a position in target, got: ${JSON.stringify(target)}`);
  }
  return `${type}-${target.lineId}-${target.position}-link`;
}

/**
 * Generate an accessible aria-label for the link.
 *
 * Format: "Select [type] [displayText]"
 * Example: "Select station 2:5"
 */
export function generateAriaLabel(type: DetailLinkType, displayText: string): string {
  const trimmed = displayText.trim();
  if (trimmed === '') {
    return `Select ${type}`;
  }
  return `Select ${type} ${trimmed}`;
}

/**
 * DetailLink component for navigating between survey elements.
 */
export function DetailLink({
  type,
  target,
  displayText,
  onSelect,
  isMissing = false,
  onAnnounce,
  testId,
  className = '',
  onHoverStart,
  onHoverEnd,
  pillLabel,
}: DetailLinkProps) {
  const buttonRef = useRef<HTMLButtonElement>(null);
  const mouseDownRef = useRef(false);
  const computedTestId = testId ?? generateTestId(type, target);
  const ariaLabel = generateAriaLabel(type, displayText);

  // Full highlighting: a leading entity symbol signals what the link points to
  // (station ring, shot line, line zigzag), colored by entity. Hidden for
  // missing targets (the warning glyph already conveys state).
  const syntacticLevel = useUIStore((s) => s.syntacticLevel);
  const showSymbol = syntacticLevelAtLeast(syntacticLevel, 'full') && !isMissing;

  const handleClick = useCallback(() => {
    if (isMissing) {
      // Don't navigate to missing targets
      return;
    }
    const shouldClearHover = mouseDownRef.current;
    mouseDownRef.current = false;
    if (shouldClearHover && onHoverEnd) {
      onHoverEnd();
    }
    onSelect();
    if (onAnnounce) {
      onAnnounce(`Selected ${type} ${displayText}`);
    }
  }, [type, displayText, onSelect, onAnnounce, isMissing, onHoverEnd]);

  const handleMouseEnter = useCallback(() => {
    if (isMissing || !onHoverStart) return;
    onHoverStart();
  }, [isMissing, onHoverStart]);

  const handleMouseDown = useCallback(() => {
    if (isMissing) return;
    mouseDownRef.current = true;
  }, [isMissing]);

  const handleMouseLeave = useCallback(() => {
    // Skip for missing links (they never started hover, so nothing to clear)
    if (isMissing || !onHoverEnd) return;
    mouseDownRef.current = false;
    // Only clear highlight if element is not focused (keyboard focus takes precedence)
    const isFocused = typeof document !== 'undefined' && document.activeElement === buttonRef.current;
    if (!isFocused) {
      onHoverEnd();
    }
  }, [isMissing, onHoverEnd]);

  const handleFocus = useCallback(() => {
    if (isMissing || !onHoverStart) return;
    onHoverStart();
  }, [isMissing, onHoverStart]);

  const handleBlur = useCallback(() => {
    // Skip for missing links (they never started hover, so nothing to clear)
    if (isMissing || !onHoverEnd) return;
    onHoverEnd();
  }, [isMissing, onHoverEnd]);

  // Build class list conditionally to avoid conflicting Tailwind utilities
  const focusClasses = 'focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset';
  const cursorClass = isMissing ? 'cursor-not-allowed' : 'cursor-pointer';

  // In Full highlighting, render the link as an entity pill: the duotone symbol
  // plus the id inside a rounded chip with a tinted background and an
  // entity-colored border (the legend-chip look). Otherwise keep the plain
  // underline link.
  const computedClass = showSymbol
    ? `sh-pill inline-flex items-center gap-1 align-middle rounded-full px-2 py-0.5 text-xs leading-none text-text-primary ${focusClasses} ${cursorClass} ${className}`
    : `underline ${isMissing ? 'text-warning' : 'text-accent'} ${focusClasses} ${cursorClass} ${className}`;
  const pillStyle: CSSProperties | undefined = showSymbol
    ? ({ '--o': `var(--sh-${type})` } as CSSProperties)
    : undefined;

  // Pill content: a short primary label + the fully-qualified id after a divider,
  // both at the same text size (shot "55 | 110:55", line "Main Line | 110").
  let pillPrimary = displayText;
  let pillId: string | null = null;
  if (showSymbol) {
    if (hasPosition(target)) {
      pillPrimary = String(target.position);
      pillId = displayText; // already the formatted "lineId:position"
    } else {
      pillPrimary = pillLabel ?? displayText;
      pillId = String(target.lineId);
    }
  }

  return (
    <button
      ref={buttonRef}
      type="button"
      onClick={handleClick}
      onMouseDown={handleMouseDown}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
      onFocus={handleFocus}
      onBlur={handleBlur}
      className={computedClass}
      style={pillStyle}
      aria-label={ariaLabel}
      aria-disabled={isMissing}
      data-testid={computedTestId}
      title={isMissing ? `${type} not found` : undefined}
    >
      {showSymbol ? (
        <>
          <EntitySymbol entity={type} size={14} testId={`${computedTestId}-symbol`} />
          <span>{pillPrimary}</span>
          {pillId !== null && (
            <span className="sh-pill-id" data-testid={`${computedTestId}-id`}>
              {/* The id suffix is always cap-height digits, so it gets the same
                  downward nudge whether the primary is a digit (shot/station) or
                  a lowercase name (line). */}
              <span className="sh-pill-nudge sh-pill-nudge--num">
                {pillId}
              </span>
            </span>
          )}
        </>
      ) : (
        <>
          {displayText}
          {isMissing && (
            <span className="ml-1" aria-hidden="true" title={`${type} not found`}>
              &#x26A0;
            </span>
          )}
        </>
      )}
    </button>
  );
}

export default DetailLink;
