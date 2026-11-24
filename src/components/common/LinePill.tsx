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
 * shown on a line that is not its home line. A loop end (a station a loop
 * closure shows as its other end, RS 6) carries a small loop mark there too,
 * even where it has no dot. A shot pill carries each end's marks after that
 * end's name. The marks stand for the qualifier plain text adds (`12 (Main)`,
 * `3 (loop end on Side)`), so a pill's names are bare; each mark carries those
 * words for a screen reader, and the home-line mark a tooltip ("Belongs to
 * Main", RS-FX6 P2-5), as the loop mark does.
 */
import { useCallback, type CSSProperties, type ReactNode } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import {
  missingPillColorsOf,
  pillColorsOf,
  pillMarkOf,
  pillThemeOf,
  prosePillColorsOf,
  proseTintOf,
  type MissingPillColors,
  type PillColors,
  type PillDot,
} from '../../theme/pillColors';
import { SHOT_ARROW } from '../../utils/formatters';
import i18n from '../../i18n';

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

/** The name of line `lineId`, or null when there is no such line. */
export function useLineNameOf(): (lineId: number | null | undefined) => string | null {
  const { projectLines, dataLines } = useProjectStore(
    useShallow((s) => ({ projectLines: s.project?.lines, dataLines: s.surveyData?.lines })),
  );
  return useCallback(
    (lineId) => {
      if (lineId === null || lineId === undefined) return null;
      return (
        projectLines?.find((l) => l.id === lineId)?.name ?? dataLines?.find((l) => l.id === lineId)?.name ?? null
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
  /** A line's name, for the words a home-line mark carries. */
  lineNameOf?: ((lineId: number | null | undefined) => string | null) | undefined;
}

/** The colours of the pill of line `lineId` on the current theme. */
export function usePillColors(lineId: number | null | undefined): ResolvedPill {
  const theme = useUIStore((s) => s.resolvedTheme);
  const colorOf = useLineColorOf();
  const lineNameOf = useLineNameOf();
  return { colors: pillColorsOf(colorOf(lineId), theme), theme, colorOf, lineNameOf };
}

/** The colours of the prose pill of line `lineId` on the current theme (RS-PROSE, `prosePillColors`). */
export function useProsePillColors(lineId: number | null | undefined): ResolvedPill {
  const theme = useUIStore((s) => s.resolvedTheme);
  const colorOf = useLineColorOf();
  const lineNameOf = useLineNameOf();
  return { colors: prosePillColorsOf(colorOf(lineId), theme), theme, colorOf, lineNameOf };
}

/**
 * A prose pill's colours as the custom properties the `.sh-ppill` rules read:
 * the tint as a translucent overlay that gives the tint's centre over the
 * panel (`proseTintOf`), the same colour fully transparent (what the radial
 * tint fades to), and the text, muted part, dot and ring.
 */
export function prosePillStyle(colors: PillColors, themeName: string): CSSProperties {
  const { tint, clear } = proseTintOf(colors.bg, pillThemeOf(themeName).panel);
  return {
    '--pill-tint': tint,
    '--pill-tint-clear': clear,
    '--pill-text': colors.text,
    '--pill-muted': colors.muted,
    '--pill-dot': colors.dot,
    '--pill-ring': colors.ring ?? 'transparent',
  } as CSSProperties;
}

/** A home-line mark: its dot, and the name of the line it stands for. */
export type HomeMark = PillDot & { line?: string | undefined };

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
export function markOf(pill: ResolvedPill, homeLineId: number | null | undefined): HomeMark | null {
  if (homeLineId === null || homeLineId === undefined) return null;
  const line = pill.lineNameOf?.(homeLineId) ?? undefined;
  return { ...pillMarkOf(pill.colorOf(homeLineId), pill.colors, pill.theme), line };
}

/**
 * The second dot after a station name (see the module doc). With the home
 * line's name it says it in words too: a tooltip, "Belongs to Main", and
 * for a screen reader " (Main)", so the name reads "12 (Main)" as plain text
 * writes it (RS 6; RS-FX6, P2-5). Before, the 5 px dot was the only sign.
 */
export function HomeLineMark({ mark, testId }: { mark: HomeMark; testId?: string | undefined }) {
  const style = { '--mark-dot': mark.dot, '--mark-ring': mark.ring ?? 'transparent' } as CSSProperties;
  if (!mark.line) {
    return <span className="sh-lpill-mark" style={style} aria-hidden="true" data-testid={testId} />;
  }
  return (
    <span
      className="sh-lpill-mark"
      style={style}
      title={i18n.t('identity.homeMarkTitle', { line: mark.line })}
      data-testid={testId}
    >
      <span className="sr-only"> {i18n.t('identity.homeMarkLabel', { line: mark.line })}</span>
    </span>
  );
}

/** The loop mark after a loop end's name (see the module doc): a small open
 *  ring in the pill's muted colour, with words for a screen reader. */
export function LoopEndMark({ label, testId }: { label?: string | undefined; testId?: string | undefined }) {
  return (
    <span className="sh-lpill-loop" data-testid={testId}>
      <LoopGlyph />
      {label && <span className="sr-only"> {label}</span>}
    </span>
  );
}

/** The loop mark's glyph (an open ring with an arrowhead) in `currentColor`, 1em square. */
export function LoopGlyph() {
  return (
    <svg viewBox="0 0 12 12" width="1em" height="1em" aria-hidden="true" focusable="false">
      <path d="M9.6 4.2A4 4 0 1 0 10 6" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
      <path d="M11.2 2.4 L10.4 5.4 L7.6 4.4 Z" fill="currentColor" />
    </svg>
  );
}

/** A shot's two station names, each with its home-line mark and its loop
 *  mark when it has one (a station pill's marks, per end). */
export interface ShotEnds {
  from: string;
  to: string;
  fromMark?: HomeMark | null | undefined;
  toMark?: HomeMark | null | undefined;
  fromLoopEnd?: boolean | undefined;
  toLoopEnd?: boolean | undefined;
}

export interface PillPartsProps {
  /** Test id root: parts get `-dot`, `-id` (the context), `-home-dot`, `-loop-end`, `-station-id`,
   *  and a shot's ends `-from-home-dot`, `-from-loop-end`, `-to-home-dot`, `-to-loop-end`. */
  testId?: string | undefined;
  /** Muted context before the main part, shown with a colon (`Main Line:`). */
  context?: string | null | undefined;
  /** The main part: a station name, a shot, a line name. */
  main: ReactNode;
  /** A shot pill's main part as its two ends, so each can carry a mark. Overrides `main`. */
  shotEnds?: ShotEnds | null | undefined;
  /** A station pill's home-line mark, right after the name. */
  mark?: HomeMark | null | undefined;
  /** A station pill's loop mark (a loop end, RS 6), after the name and its dot. */
  loopEnd?: boolean | undefined;
  /** Words read for a loop mark (the station's, or a shot end's) by a screen reader. */
  loopEndLabel?: string | undefined;
  /** A station's Station ID, as a muted superscript at the end (`#42`; the `#` is not read aloud). */
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
  loopEnd = false,
  loopEndLabel,
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
            {shotEnds.fromLoopEnd && <LoopEndMark label={loopEndLabel} testId={part('from-loop-end')} />}
            {` ${SHOT_ARROW} `}
            {shotEnds.to}
            {shotEnds.toMark && <HomeLineMark mark={shotEnds.toMark} testId={part('to-home-dot')} />}
            {shotEnds.toLoopEnd && <LoopEndMark label={loopEndLabel} testId={part('to-loop-end')} />}
          </>
        ) : (
          main
        )}
      </span>
      {mark && <HomeLineMark mark={mark} testId={part('home-dot')} />}
      {loopEnd && <LoopEndMark label={loopEndLabel} testId={part('loop-end')} />}
      {stationId !== null && stationId !== undefined && (
        <>
          {' '}
          <sup className="sh-lpill-sid" data-testid={part('station-id')}>
            {stationIdLabel && <span className="sr-only">{stationIdLabel} </span>}
            <span aria-hidden="true">#</span>
            {stationId}
          </sup>
        </>
      )}
      {trailing}
    </>
  );
}
