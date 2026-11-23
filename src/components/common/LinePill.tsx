/**
 * The body of a line link pill, shared by `DetailLink` (a button) and
 * `EntityBlob` (a span): a dot in the line's colour, the muted context
 * (`Main Line:`), the main part at the surrounding text size (a station name,
 * a shot `4 → 5`, a line name) and, for a station, its Station ID as a muted
 * superscript. Every colour comes from `theme/pillColors.ts` through CSS
 * custom properties (the `.sh-lpill` rules in index.css).
 *
 * A station can also carry a home-line mark: a small second dot, in the colour
 * of the line the station belongs to, right after its name, for a station
 * shown on a line that is not its home line.
 */
import { useCallback, type CSSProperties, type ReactNode } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import {
  missingPillColorsOf,
  pillColorsOf,
  pillMarkOf,
  type MissingPillColors,
  type PillColors,
  type PillDot,
} from '../../theme/pillColors';
import { SHOT_ARROW } from '../../utils/formatters';

/** The colour of line `lineId` (any CSS colour), or null when there is no such line. */
export function useLineColorOf(): (lineId: number | null | undefined) => string | null {
  const { projectLines, dataLines } = useProjectStore(
    useShallow((s) => ({ projectLines: s.project?.lines, dataLines: s.surveyData?.lines })),
  );
  return useCallback(
    (lineId) => {
      if (lineId === null || lineId === undefined) return null;
      return (
        projectLines?.find((l) => l.id === lineId)?.color ?? dataLines?.find((l) => l.id === lineId)?.color ?? null
      );
    },
    [projectLines, dataLines],
  );
}

export interface ResolvedPill {
  colors: PillColors;
  /** The theme name the colours were derived for (marks are derived on it too). */
  theme: string;
  colorOf: (lineId: number | null | undefined) => string | null;
}

/** The colours of the pill of line `lineId` on the current theme. */
export function usePillColors(lineId: number | null | undefined): ResolvedPill {
  const theme = useUIStore((s) => s.resolvedTheme);
  const colorOf = useLineColorOf();
  return { colors: pillColorsOf(colorOf(lineId), theme), theme, colorOf };
}

/** The pill's colours as the custom properties the `.sh-lpill` rules read. */
export function pillStyle(colors: PillColors): CSSProperties {
  return {
    '--pill-bg': colors.bg,
    '--pill-border': colors.border,
    '--pill-bg-hover': colors.hoverBg,
    '--pill-border-hover': colors.hoverBorder,
    '--pill-text': colors.text,
    '--pill-muted': colors.muted,
    '--pill-dot': colors.dot,
    '--pill-ring': colors.ring ?? 'transparent',
  } as CSSProperties;
}

/** A missing target's pill: neutral, no hover change. */
export function missingPillStyle(colors: MissingPillColors): CSSProperties {
  return {
    '--pill-bg': colors.bg,
    '--pill-border': colors.border,
    '--pill-bg-hover': colors.bg,
    '--pill-border-hover': colors.border,
    '--pill-text': colors.text,
    '--pill-muted': colors.muted,
    '--pill-warning': colors.warning,
  } as CSSProperties;
}

/** The current theme's missing-target pill colours. */
export function useMissingPillStyle(): CSSProperties {
  const theme = useUIStore((s) => s.resolvedTheme);
  return missingPillStyle(missingPillColorsOf(theme));
}

/** The home-line mark's dot for line `homeLineId` on a pill coloured `pill`, or null for none. */
export function markOf(pill: ResolvedPill, homeLineId: number | null | undefined): PillDot | null {
  if (homeLineId === null || homeLineId === undefined) return null;
  return pillMarkOf(pill.colorOf(homeLineId), pill.colors, pill.theme);
}

/** The second dot after a station name (see the module doc). */
export function HomeLineMark({ mark, testId }: { mark: PillDot; testId?: string | undefined }) {
  return (
    <span
      className="sh-lpill-mark"
      style={{ '--mark-dot': mark.dot, '--mark-ring': mark.ring ?? 'transparent' } as CSSProperties}
      aria-hidden="true"
      data-testid={testId}
    />
  );
}

/** A shot's two station names, each with its home-line mark when it has one. */
export interface ShotEnds {
  from: string;
  to: string;
  fromMark?: PillDot | null | undefined;
  toMark?: PillDot | null | undefined;
}

export interface PillPartsProps {
  /** Test id root: parts get `-dot`, `-id` (the context), `-home-dot`, `-station-id`. */
  testId?: string | undefined;
  /** Muted context before the main part, shown with a colon (`Main Line:`). */
  context?: string | null | undefined;
  /** The main part: a station name, a shot, a line name. */
  main: ReactNode;
  /** A shot pill's main part as its two ends, so each can carry a mark. Overrides `main`. */
  shotEnds?: ShotEnds | null | undefined;
  /** A station pill's home-line mark, right after the name. */
  mark?: PillDot | null | undefined;
  /** A station's Station ID, as a muted superscript at the end. */
  stationId?: number | null | undefined;
  /** Words read before the Station ID by a screen reader (a span pill has no aria-label). */
  stationIdLabel?: string | undefined;
  /** The target is gone: a warning glyph instead of the dot. */
  missing?: boolean | undefined;
  /** After everything (the data table's picker chevron). */
  trailing?: ReactNode | undefined;
}

/** The inside of a line pill; the caller renders the `.sh-lpill` element around it. */
export function PillParts({
  testId,
  context,
  main,
  shotEnds,
  mark,
  stationId,
  stationIdLabel,
  missing = false,
  trailing,
}: PillPartsProps) {
  const part = (suffix: string) => (testId ? `${testId}-${suffix}` : undefined);
  return (
    <>
      {missing ? (
        <span className="sh-lpill-warn" aria-hidden="true" data-testid={part('missing')}>
          &#x26A0;
        </span>
      ) : (
        <span className="sh-lpill-dot" aria-hidden="true" data-testid={part('dot')} />
      )}
      {context ? (
        <>
          <span className="sh-lpill-ctx" data-testid={part('id')}>
            {context}:
          </span>{' '}
        </>
      ) : null}
      <span className="sh-lpill-main">
        {shotEnds ? (
          <>
            {shotEnds.from}
            {shotEnds.fromMark && <HomeLineMark mark={shotEnds.fromMark} testId={part('from-home-dot')} />}
            {` ${SHOT_ARROW} `}
            {shotEnds.to}
            {shotEnds.toMark && <HomeLineMark mark={shotEnds.toMark} testId={part('to-home-dot')} />}
          </>
        ) : (
          main
        )}
      </span>
      {mark && <HomeLineMark mark={mark} testId={part('home-dot')} />}
      {stationId !== null && stationId !== undefined && (
        <>
          {' '}
          <sup className="sh-lpill-sid" data-testid={part('station-id')}>
            {stationIdLabel && <span className="sr-only">{stationIdLabel} </span>}
            {stationId}
          </sup>
        </>
      )}
      {trailing}
    </>
  );
}
