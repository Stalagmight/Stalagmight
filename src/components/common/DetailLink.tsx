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
import { useTranslation } from 'react-i18next';
import i18n from '../../i18n';
import { useUIStore } from '../../store/uiStore';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { EntitySymbol } from './toolIcons';

export type DetailLinkType = 'station' | 'shot' | 'section';

/**
 * en.json keys of each link type's text. One whole phrase per type rather than the
 * type name spliced into a sentence, so a translation can inflect the entity name.
 */
const LINK_TEXT: Record<
  DetailLinkType,
  { notFoundKey: string; selectKey: string; selectNamedKey: string; selectedKey: string }
> = {
  station: {
    notFoundKey: 'detailLink.station.notFound',
    selectKey: 'detailLink.station.select',
    selectNamedKey: 'detailLink.station.selectNamed',
    selectedKey: 'detailLink.station.selected',
  },
  shot: {
    notFoundKey: 'detailLink.shot.notFound',
    selectKey: 'detailLink.shot.select',
    selectNamedKey: 'detailLink.shot.selectNamed',
    selectedKey: 'detailLink.shot.selected',
  },
  section: {
    notFoundKey: 'detailLink.section.notFound',
    selectKey: 'detailLink.section.select',
    selectNamedKey: 'detailLink.section.selectNamed',
    selectedKey: 'detailLink.section.selected',
  },
};

/**
 * Target identifier for station or shot links.
 * Both sectionId and position are required.
 */
export interface StationOrShotTarget {
  sectionId: number;
  /** Position within section: 0-indexed for stations, 1-indexed for shots */
  position: number;
}

/**
 * Target identifier for section links.
 * Only sectionId is required.
 */
export interface SectionTarget {
  sectionId: number;
}

/**
 * Union type for all target types.
 * Use StationOrShotTarget for stations/shots, SectionTarget for sections.
 */
export type DetailLinkTarget = StationOrShotTarget | SectionTarget;

/**
 * Type guard to check if target has position (station or shot target).
 */
export function hasPosition(target: DetailLinkTarget): target is StationOrShotTarget {
  return 'position' in target && typeof target.position === 'number';
}

export interface DetailLinkProps {
  /** Type of element this link navigates to */
  type: DetailLinkType;
  /** Target identifier (sectionId required, position required for stations/shots) */
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
   * suffix. Used for sections (the bare name, e.g. "Main Line"); station/shot
   * links derive their primary from the target position automatically.
   */
  pillLabel?: string;
}

/**
 * Generate a stable test ID based on element type and target.
 *
 * Formats:
 * - Station: "station-{sectionId}-{position}-link"
 * - Shot: "shot-{sectionId}-{position}-link"
 * - Section: "section-{sectionId}-link"
 *
 * @throws Error if station/shot type is used without position
 */
export function generateTestId(type: DetailLinkType, target: DetailLinkTarget): string {
  if (type === 'section') {
    return `section-${target.sectionId}-link`;
  }
  // Station and shot require position
  if (!hasPosition(target)) {
    throw new Error(`${type} links require a position in target, got: ${JSON.stringify(target)}`);
  }
  return `${type}-${target.sectionId}-${target.position}-link`;
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
    return i18n.t(LINK_TEXT[type].selectKey);
  }
  return i18n.t(LINK_TEXT[type].selectNamedKey, { name: trimmed });
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
  const { t } = useTranslation();
  const buttonRef = useRef<HTMLButtonElement>(null);
  const mouseDownRef = useRef(false);
  const computedTestId = testId ?? generateTestId(type, target);
  const ariaLabel = generateAriaLabel(type, displayText);

  // Full highlighting: a leading entity symbol signals what the link points to
  // (station ring, shot line, section zigzag), colored by entity. Hidden for
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
      onAnnounce(t(LINK_TEXT[type].selectedKey, { name: displayText }));
    }
  }, [type, displayText, onSelect, onAnnounce, isMissing, onHoverEnd, t]);

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
  // both at the same text size (shot "55 | 110:55", section "Main Line | 110").
  let pillPrimary = displayText;
  let pillId: string | null = null;
  if (showSymbol) {
    if (hasPosition(target)) {
      pillPrimary = String(target.position);
      pillId = displayText; // already the formatted "sectionId:position"
    } else {
      pillPrimary = pillLabel ?? displayText;
      pillId = String(target.sectionId);
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
      title={isMissing ? t(LINK_TEXT[type].notFoundKey) : undefined}
    >
      {showSymbol ? (
        <>
          <EntitySymbol entity={type} size={14} testId={`${computedTestId}-symbol`} />
          <span>{pillPrimary}</span>
          {pillId !== null && (
            <span className="sh-pill-id" data-testid={`${computedTestId}-id`}>
              {/* The id suffix is always cap-height digits, so it gets the same
                  downward nudge whether the primary is a digit (shot/station) or
                  a lowercase name (section). */}
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
            <span className="ml-1" aria-hidden="true" title={t(LINK_TEXT[type].notFoundKey)}>
              &#x26A0;
            </span>
          )}
        </>
      )}
    </button>
  );
}

export default DetailLink;
