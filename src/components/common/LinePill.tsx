/**
 * The body of a line link pill, shared by `DetailLink` (a button) and
 * `EntityBlob` (a span): a dot in the line's colour, the muted context
 * (`Main Line:`), the main part at the surrounding text size (a station name,
 * a shot `4 → 5`, a line name) and, for a station, its Station ID as a muted
 * superscript. Every colour comes from `theme/pillColors.ts` through CSS
 * custom properties (the `.sh-lpill` rules in index.css).
 *
 * A station pill takes the colour of the line its shown station belongs to
 * (RS-OWNERSHIP): a foreign slot (Side's junction showing Main's 12) or a
 * loop end whose kept end lives on another line is a Main pill, its dot
 * Main's and its context "Main:", where it once carried a second, smaller
 * dot. Its words for a screen reader and its tooltip ("Belongs to Main")
 * say the same. A loop end keeps its loop mark after the name.
 *
 * A shot pill with an end whose station belongs to another line is two-tone
 * ({@link ShotTone}): split at the arrow, the half holding that end takes
 * that line's colour and the other half keeps the shot's; the foreign end
 * keeps its home-line dot after its name, and a loop end its loop mark.
 * When the from end is the foreign one, the leading dot and the context
 * (the shot's line) keep the shot's colour on their own, so no half reads
 * one line's name in another's colour.
 */
import { useCallback, type CSSProperties, type ReactNode } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import {
  missingPillColorsOf,
  pillColorsOf,
  pillMarkOf,
  pillMarkOver,
  pillThemeOf,
  prosePillColorsOf,
  proseTintOf,
  type MissingPillColors,
  type PillColors,
  type PillDot,
} from '../../theme/pillColors';
import { SHOT_ARROW } from '../../utils/formatters';
import { blendBackdrops, linkSegmentStyle, proseSegmentStyle } from '../../theme/pillBlend';
import { lineDisplayName } from '../../utils/stationIdentity';
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

/**
 * The display name of line `lineId` (`lineDisplayName`: bidi and control
 * characters dropped, "Unnamed line" for none, RS-PROSE fix 6), or null when
 * there is no such line.
 */
export function useLineNameOf(): (lineId: number | null | undefined) => string | null {
  const { projectLines, dataLines } = useProjectStore(
    useShallow((s) => ({ projectLines: s.project?.lines, dataLines: s.surveyData?.lines })),
  );
  return useCallback(
    (lineId) => {
      if (lineId === null || lineId === undefined) return null;
      const line = projectLines?.find((l) => l.id === lineId) ?? dataLines?.find((l) => l.id === lineId);
      return line === undefined ? null : lineDisplayName(line.name);
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

/**
 * Any line's pill on the current theme, for a pill that paints more than one
 * line (a two-tone shot pill). `prose`: the prose pill's colours.
 */
export function useResolvePill(variant: 'link' | 'prose' = 'link'): (lineId: number | null | undefined) => ResolvedPill {
  const theme = useUIStore((s) => s.resolvedTheme);
  const colorOf = useLineColorOf();
  const lineNameOf = useLineNameOf();
  return useCallback(
    (lineId) => ({
      colors: (variant === 'prose' ? prosePillColorsOf : pillColorsOf)(colorOf(lineId), theme),
      theme,
      colorOf,
      lineNameOf,
    }),
    [variant, theme, colorOf, lineNameOf],
  );
}

/**
 * The styles of a two-tone shot pill's segments (RS-OWNERSHIP), blended at
 * each seam (RS-BLEND, `theme/pillBlend.ts`), or null for a one-colour shot
 * (neither end belongs to another line).
 */
export interface ShotTone {
  /** The leading dot and the context, in the shot's colour, on their own:
   *  only when the from end's half takes another line's colour. */
  head: CSSProperties | null;
  /** The from end's half: its owner's colour, or the shot's. */
  from: CSSProperties;
  /** The to end's half, from the arrow on: its owner's colour, or the shot's. */
  to: CSSProperties;
  /** What the from end's dot can sit on (its half's colours and blends). */
  fromOn: string[];
  /** What the to end's dot can sit on. */
  toOn: string[];
}

/** The words of a two-tone shot's segments, for a prose tint's seam width. */
export interface ShotWords {
  head: string;
  from: string;
  to: string;
}

/** About how wide `words` are in em (a pill's average glyph, its padding and a dot). */
const widthEmOf = (words: string) => words.length * 0.56 + 0.9;

/**
 * {@link ShotTone} of a shot of line `lineId` whose ends belong to
 * `fromOwner` / `toOwner` (null: the shot's own line), each segment's
 * colours from `resolve` (a link or a prose pill's) and blended with its
 * neighbours: a link pill's fill and border gradients, a prose pill's tint
 * (`words` sizes its seam fade).
 */
export function shotToneOf(
  resolve: (lineId: number | null | undefined) => ResolvedPill,
  lineId: number | null,
  fromOwner: number | null | undefined,
  toOwner: number | null | undefined,
  variant: 'link' | 'prose' = 'link',
  words?: ShotWords,
): ShotTone | null {
  const foreign = (owner: number | null | undefined) => owner !== null && owner !== undefined && owner !== lineId;
  if (!foreign(fromOwner) && !foreign(toOwner)) return null;
  const shot = resolve(lineId);
  const lines = [
    ...(foreign(fromOwner) ? [{ key: 'head' as const, pill: shot }] : []),
    { key: 'from' as const, pill: foreign(fromOwner) ? resolve(fromOwner) : shot },
    { key: 'to' as const, pill: foreign(toOwner) ? resolve(toOwner) : shot },
  ];
  const panel = pillThemeOf(shot.theme).panel;
  const styles: Partial<Record<'head' | 'from' | 'to', CSSProperties>> = {};
  const on: Partial<Record<'from' | 'to', string[]>> = {};
  lines.forEach((seg, i) => {
    const near = { left: lines[i - 1]?.pill.colors ?? null, right: lines[i + 1]?.pill.colors ?? null };
    styles[seg.key] =
      variant === 'link'
        ? linkSegmentStyle(seg.pill.colors, near, seg.pill.theme)
        : proseSegmentStyle(seg.pill.colors, near, seg.pill.theme, widthEmOf(words?.[seg.key] ?? ''));
    if (seg.key !== 'head') {
      on[seg.key] = blendBackdrops(seg.pill.colors, near, variant === 'prose' ? panel : undefined);
    }
  });
  return {
    head: styles.head ?? null,
    from: styles.from!,
    to: styles.to!,
    fromOn: on.from ?? [],
    toOn: on.to ?? [],
  };
}

/**
 * The home-line mark of line `homeLineId` drawn over `backdrops` (a
 * two-tone segment's colours and blends, {@link ShotTone}), or null for none.
 */
export function markOver(pill: ResolvedPill, homeLineId: number | null | undefined, backdrops: readonly string[]): HomeMark | null {
  if (homeLineId === null || homeLineId === undefined) return null;
  const line = pill.lineNameOf?.(homeLineId) ?? undefined;
  return { ...pillMarkOver(pill.colorOf(homeLineId), backdrops, pill.theme), line };
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
 * The dot after a foreign shot end's name (see the module doc; a station
 * pill takes its owner's colour instead since RS-OWNERSHIP). With the home
 * line's name it says it in words too: a tooltip, "Belongs to Main", and
 * for a screen reader " (Main)", so the end reads "12 (Main)" as plain text
 * writes it (RS 6; RS-FX6, P2-5).
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
 *  mark when it has one (the foreign end's dot stays, RS-OWNERSHIP). */
export interface ShotEnds {
  from: string;
  to: string;
  fromMark?: HomeMark | null | undefined;
  toMark?: HomeMark | null | undefined;
  fromLoopEnd?: boolean | undefined;
  toLoopEnd?: boolean | undefined;
}

export interface PillPartsProps {
  /** Test id root: parts get `-dot`, `-id` (the context), `-loop-end`, `-station-id`,
   *  `-owner` (a station's owner words), and a shot's ends `-from-home-dot`,
   *  `-from-loop-end`, `-to-home-dot`, `-to-loop-end`; a two-tone shot's
   *  segments `-head`, `-from`, `-to`. */
  testId?: string | undefined;
  /** Muted context before the main part, shown with a colon (`Main Line:`). */
  context?: string | null | undefined;
  /** The main part: a station name, a shot, a line name. */
  main: ReactNode;
  /** A shot pill's main part as its two ends, so each can carry a mark. Overrides `main`. */
  shotEnds?: ShotEnds | null | undefined;
  /** A two-tone shot pill's segment styles (RS-OWNERSHIP); the caller gives the
   *  pill `sh-lpill--split` and no colours of its own. Needs `shotEnds`. */
  tone?: ShotTone | null | undefined;
  /** A station pill's loop mark (a loop end, RS 6), after the name. */
  loopEnd?: boolean | undefined;
  /** Words read for a loop mark (the station's, or a shot end's) by a screen reader. */
  loopEndLabel?: string | undefined;
  /** A station's Station ID, as a muted superscript at the end (`#42`; the `#` is not read aloud). */
  stationId?: number | null | undefined;
  /** Words read before the Station ID by a screen reader (a span pill has no aria-label). */
  stationIdLabel?: string | undefined;
  /** Words read after a station's name by a screen reader when the pill
   *  takes another line's colour (`(Main)`, RS-OWNERSHIP): what the home-line
   *  dot used to say. A button pill's aria-label says it already. */
  ownerWords?: string | null | undefined;
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
  tone,
  loopEnd = false,
  loopEndLabel,
  stationId,
  stationIdLabel,
  ownerWords,
  missing = false,
  trailing,
}: PillPartsProps) {
  const part = (suffix: string) => (testId ? `${testId}-${suffix}` : undefined);
  const dot = missing ? (
    <span className="sh-lpill-warn" aria-hidden="true" data-testid={part('missing')}>
      &#x26A0;
    </span>
  ) : (
    <span className="sh-lpill-dot" aria-hidden="true" data-testid={part('dot')} />
  );
  const ctx = context ? (
    <>
      <span className="sh-lpill-ctx" data-testid={part('id')}>
        {context}:
      </span>{' '}
    </>
  ) : null;
  const endMarks = (which: 'from' | 'to') => {
    if (!shotEnds) return null;
    const mark = which === 'from' ? shotEnds.fromMark : shotEnds.toMark;
    const loop = which === 'from' ? shotEnds.fromLoopEnd : shotEnds.toLoopEnd;
    return (
      <>
        {mark && <HomeLineMark mark={mark} testId={part(`${which}-home-dot`)} />}
        {loop && <LoopEndMark label={loopEndLabel} testId={part(`${which}-loop-end`)} />}
      </>
    );
  };
  if (shotEnds && tone) {
    // Two-tone (RS-OWNERSHIP): the split falls before the arrow; the head
    // (dot, context) stays in the shot's colour when the from end's half is
    // another line's.
    return (
      <>
        {tone.head && (
          <span className="sh-lpill-seg" style={tone.head} data-testid={part('head')}>
            {dot}
            {ctx}
          </span>
        )}
        <span className="sh-lpill-seg" style={tone.from} data-testid={part('from')}>
          {!tone.head && dot}
          {!tone.head && ctx}
          <span className="sh-lpill-main">{shotEnds.from}</span>
          {endMarks('from')}
        </span>{' '}
        <span className="sh-lpill-seg" style={tone.to} data-testid={part('to')}>
          <span className="sh-lpill-main">
            {`${SHOT_ARROW} `}
            {shotEnds.to}
          </span>
          {endMarks('to')}
          {trailing}
        </span>
      </>
    );
  }
  return (
    <>
      {dot}
      {ctx}
      <span className="sh-lpill-main">
        {shotEnds ? (
          <>
            {shotEnds.from}
            {endMarks('from')}
            {` ${SHOT_ARROW} `}
            {shotEnds.to}
            {endMarks('to')}
          </>
        ) : (
          main
        )}
      </span>
      {ownerWords && (
        <span className="sr-only" data-testid={part('owner')}>
          {' '}
          {ownerWords}
        </span>
      )}
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
