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

import { useCallback, useRef } from 'react';
import i18n from '../../i18n';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useUIStore } from '../../store/uiStore';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { displayStationName } from '../../utils/formatters';
import { markOf, PillParts, pillStyle, useMissingPillStyle, usePillColors } from './LinePill';

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
  /**
   * The target named with its line context, for the aria-label and the
   * selection announcement (`Main Line / 17`, `Main Line: 4 → 5`) when the
   * visible text omits the line. Defaults to `displayText`.
   */
  fullText?: string;
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
   * The main part of the Full-level pill, at the surrounding text size: the
   * station name (`17`), the shot (`4 → 5`) or the line name. Defaults to
   * `displayText`.
   */
  pillLabel?: string;
  /**
   * The muted, smaller context before the main part in the Full-level pill:
   * the line name of a station or shot link, shown with a colon
   * (`Main Line: 17`). Omitted for lines. Never an id - positional handles
   * are not shown (SPEC 4.1).
   */
  pillContext?: string;
  /**
   * A station pill's home-line mark: the line the station belongs to, when the
   * pill shows it on another line (a small dot in that line's colour after the
   * name). Station links only.
   */
  homeLineId?: number | null | undefined;
  /**
   * Whether a station pill ends with its Station ID (default). False where the
   * pill is labelled by something else: the Station details' "Also on" rows
   * name the other LINE (UX-7), and the Station ID row above already says it.
   */
  stationIdShown?: boolean | undefined;
  /** A shot pill's home-line marks, per end (see `homeLineId`). Shot links only. */
  shotHomeLineIds?: { from?: number | null | undefined; to?: number | null | undefined } | undefined;
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
 * The link's target as spoken: the text, then the Station ID when the pill
 * shows one. Example: "station Main Line / 17, Station ID 42".
 */
export function spokenTarget(type: DetailLinkType, displayText: string, stationId?: number | null): string {
  const trimmed = displayText.trim();
  const id = stationId === null || stationId === undefined ? '' : `, ${i18n.t('identity.stationId')} ${stationId}`;
  return trimmed === '' ? `${type}${id}` : `${type} ${trimmed}${id}`;
}

/**
 * Generate an accessible aria-label for the link.
 *
 * Format: "Select [type] [displayText][, Station ID n]"
 * Example: "Select station Main Line / 17, Station ID 42"
 */
export function generateAriaLabel(type: DetailLinkType, displayText: string, stationId?: number | null): string {
  return `Select ${spokenTarget(type, displayText, stationId)}`;
}

/**
 * DetailLink component for navigating between survey elements.
 */
export function DetailLink({
  type,
  target,
  displayText,
  fullText,
  onSelect,
  isMissing = false,
  onAnnounce,
  testId,
  className = '',
  onHoverStart,
  onHoverEnd,
  pillLabel,
  pillContext,
  homeLineId,
  stationIdShown = true,
  shotHomeLineIds,
}: DetailLinkProps) {
  const buttonRef = useRef<HTMLButtonElement>(null);
  const mouseDownRef = useRef(false);
  const computedTestId = testId ?? generateTestId(type, target);

  // Full highlighting: the link is a pill in its line's colour; below Full it
  // stays the plain underline link.
  const syntacticLevel = useUIStore((s) => s.syntacticLevel);
  const showPill = syntacticLevelAtLeast(syntacticLevel, 'full');
  const labels = useIdentityLabels();
  const pill = usePillColors(target.lineId);
  const missingStyle = useMissingPillStyle();
  const position = hasPosition(target) ? target.position : null;
  // A station pill ends with the station's Station ID (the one the Station
  // IDs label mode shows); the spoken name carries it too.
  const stationId =
    showPill && stationIdShown && !isMissing && type === 'station' && position !== null
      ? labels.stationId({ line_id: target.lineId, station_index: position })
      : null;
  const spokenText = spokenTarget(type, fullText ?? displayText, stationId);
  const ariaLabel = generateAriaLabel(type, fullText ?? displayText, stationId);

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
      onAnnounce(`Selected ${spokenText}`);
    }
  }, [spokenText, onSelect, onAnnounce, isMissing, onHoverEnd]);

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

  // Full: the line pill (`.sh-lpill`, colours from theme/pillColors.ts), a
  // missing target as its neutral dashed variant. Otherwise the plain
  // underline link.
  const computedClass = showPill
    ? `sh-lpill${isMissing ? ' sh-lpill--missing' : ''} ${focusClasses} ${cursorClass} ${className}`
    : `underline ${isMissing ? 'text-warning' : 'text-accent'} ${focusClasses} ${cursorClass} ${className}`;
  const style = showPill ? (isMissing ? missingStyle : pillStyle(pill.colors)) : undefined;

  // A shot with a home-line mark shows its two ends separately, so each mark
  // sits right after its station's name.
  const shotIndex = type === 'shot' && position !== null ? position - 1 : null;
  const fromMark = showPill && shotIndex !== null ? markOf(pill, shotHomeLineIds?.from) : null;
  const toMark = showPill && shotIndex !== null ? markOf(pill, shotHomeLineIds?.to) : null;
  const shotEnds =
    shotIndex !== null && (fromMark || toMark)
      ? {
          from: displayStationName(labels.stationName({ line_id: target.lineId, station_index: shotIndex })),
          to: displayStationName(labels.stationName({ line_id: target.lineId, station_index: shotIndex + 1 })),
          fromMark,
          toMark,
        }
      : null;

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
      style={style}
      aria-label={ariaLabel}
      aria-disabled={isMissing}
      data-testid={computedTestId}
      title={isMissing ? `${type} not found` : undefined}
    >
      {showPill ? (
        <PillParts
          testId={computedTestId}
          context={type === 'line' ? null : pillContext}
          main={pillLabel ?? displayText}
          shotEnds={shotEnds}
          mark={type === 'station' && !isMissing ? markOf(pill, homeLineId) : null}
          stationId={stationId}
          missing={isMissing}
        />
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
