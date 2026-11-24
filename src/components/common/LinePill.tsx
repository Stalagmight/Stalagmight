/**
 * The body of a line link pill, shared by `DetailLink` (a button) and
 * `EntityBlob` (a span): a dot in the line's colour, the muted context
 * (`Main Line:`), the main part at the surrounding text size (a station
 * name, a shot `4 → 5`, a line name) and, for a station, its Station ID as a
 * muted superscript. The pill shows its line by the leading dot, its fill and
 * its border (RS-LEADDOT brought the dot back after RS-NODOTS); every colour
 * comes from `theme/pillColors.ts` through CSS custom properties (the
 * `.sh-lpill` rules in index.css). The dots are decorative (aria-hidden): the
 * words say the same.
 *
 * A station pill takes the colour of the line its shown station belongs to
 * (RS-OWNERSHIP): a foreign slot (Side's junction showing Main's 12) or a
 * loop end whose kept end lives on another line is a Main pill, its dot
 * Main's and its context "Main:". Its tooltip ("Belongs to Main") and its
 * words for a screen reader say so; a loop end says "loop end" the same way.
 *
 * A station shown on a line it does not belong to (a foreign station) has a
 * second, smaller dot in its owner's colour raised to superscript height
 * right after its name, the **home dot** (RS-LEADDOT, the user: "for foreign
 * stations, instead of dot after, just put a small color dot in superscript
 * right after the station name, it doesnt need to take any space
 * horizontally"): it takes no room, so nothing before or after it moves,
 * except a Station ID right after it, which steps aside just enough to clear
 * it.
 *
 * A shot pill with an end whose station belongs to another line is two-tone
 * ({@link ShotTone}): the half holding that end takes that line's colour and
 * the other half keeps the shot's, blending across the arrow; the foreign
 * end has its home dot after its name, on its own half. A foreign or loop
 * end also says what it is in a tooltip and in words for a screen reader.
 * The leading dot is the shot's line's: when the from end is the foreign
 * one, the dot and the context keep the shot's colour on their own (a head),
 * so the dot sits in the shot's part and no half reads one line's name in
 * another's colour. The loop-end mark stays gone (RS-NODOTS).
 */
import { useCallback, type CSSProperties, type ReactNode } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import {
  missingPillColorsOf,
  pillColorsOf,
  pillThemeOf,
  prosePillColorsOf,
  proseTintOf,
  type MissingPillColors,
  type PillColors,
  type PillDot,
} from '../../theme/pillColors';
import { SHOT_ARROW } from '../../utils/formatters';
import { homeDotStyle, linkBridgeStyle, linkSegmentStyle, proseBridgeStyle, proseSegmentStyle } from '../../theme/pillBlend';
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
  /** The theme name the colours were derived for. */
  theme: string;
  colorOf: (lineId: number | null | undefined) => string | null;
  /** A line's name, for the tooltip and words of an end of another line's station. */
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
 * the glow as a wash that gives its colour over the panel (`proseTintOf`,
 * RS-NODOTS), the text and muted part, and the leading dot and its ring
 * (RS-LEADDOT), and a foreign station's home dot (the pill is its owner's;
 * the dot at its own, lower floor, `PillColors.home`).
 */
export function prosePillStyle(colors: PillColors, themeName: string): CSSProperties {
  return {
    '--pill-tint': proseTintOf(colors.bg, pillThemeOf(themeName).panel).tint,
    '--pill-text': colors.text,
    '--pill-muted': colors.muted,
    '--pill-dot': colors.dot,
    '--pill-ring': colors.ring ?? 'transparent',
    '--home-dot': colors.home.dot,
    '--home-ring': colors.home.ring ?? 'transparent',
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
 * The styles of a two-tone shot pill's segments (RS-OWNERSHIP), blended
 * (RS-BLEND, `theme/pillBlend.ts`): the arrow is a segment of its own whose
 * colour runs from the from half's to the to half's, so the halves blend
 * across the arrow (RS-ARROW); a head blends into the from half at their
 * seam. Null for a one-colour shot (neither end belongs to another line).
 */
export interface ShotTone {
  /** The leading dot and the context, in the shot's colour, on their own:
   *  only when the from end's half takes another line's colour. */
  head: CSSProperties | null;
  /** The from end's half: its owner's colour, or the shot's. */
  from: CSSProperties;
  /** The arrow: a blend from the from half's colour to the to half's. */
  arrow: CSSProperties;
  /** The to end's half, after the arrow: its owner's colour, or the shot's. */
  to: CSSProperties;
  /** Which ends belong to another line: each has the home dot after its
   *  name (its half's style carries its colours, `--home-dot`). */
  ends: { from: boolean; to: boolean };
  /** The segments in order, with their colours (a bridge's run to
   *  `bridgeTo`): a prose pill paints each line's piece of its glow from
   *  them once laid out (`useProseGlow`). */
  segments: ToneSegment[];
  /** The panel the pill sits on (a prose glow is a wash over it). */
  panel: string;
}

/** A two-tone shot pill's segments: the head (the dot and context, when the from half is another line's), the from half, the arrow, the to half. */
export type SegmentKey = 'head' | 'from' | 'arrow' | 'to';

/** One segment of a two-tone shot: its colours, and for the arrow the colours it runs to. */
export interface ToneSegment {
  key: SegmentKey;
  colors: PillColors;
  bridgeTo?: PillColors | undefined;
}

/** The colours at a segment's left and right edges. */
export const leftEdgeOf = (seg: ToneSegment): PillColors => seg.colors;
export const rightEdgeOf = (seg: ToneSegment): PillColors => seg.bridgeTo ?? seg.colors;

/**
 * {@link ShotTone} of a shot of line `lineId` whose ends belong to
 * `fromOwner` / `toOwner` (null: the shot's own line), each segment's
 * colours from `resolve` (a link or a prose pill's): a link pill's fill and
 * border gradients blended with its neighbours, a prose pill's text colours
 * (its glow is painted once laid out), and a foreign end's home dot
 * (`homeDotStyle`, checked against its half and, for the from end, the start
 * of the arrow it overhangs).
 */
export function shotToneOf(
  resolve: (lineId: number | null | undefined) => ResolvedPill,
  lineId: number | null,
  fromOwner: number | null | undefined,
  toOwner: number | null | undefined,
  variant: 'link' | 'prose' = 'link',
): ShotTone | null {
  const foreign = (owner: number | null | undefined) => owner !== null && owner !== undefined && owner !== lineId;
  if (!foreign(fromOwner) && !foreign(toOwner)) return null;
  const shot = resolve(lineId);
  const fromPill = foreign(fromOwner) ? resolve(fromOwner) : shot;
  const toPill = foreign(toOwner) ? resolve(toOwner) : shot;
  const segs: Array<ToneSegment & { pill: ResolvedPill }> = [
    ...(foreign(fromOwner) ? [{ key: 'head' as const, pill: shot, colors: shot.colors }] : []),
    { key: 'from', pill: fromPill, colors: fromPill.colors },
    { key: 'arrow', pill: fromPill, colors: fromPill.colors, bridgeTo: toPill.colors },
    { key: 'to', pill: toPill, colors: toPill.colors },
  ];
  const panel = pillThemeOf(shot.theme).panel;
  const styles: Partial<Record<SegmentKey, CSSProperties>> = {};
  segs.forEach((seg, i) => {
    const prev = segs[i - 1];
    const next = segs[i + 1];
    if (seg.bridgeTo) {
      styles.arrow =
        variant === 'link'
          ? linkBridgeStyle(seg.colors, seg.bridgeTo, seg.pill.theme)
          : proseBridgeStyle(seg.colors, seg.bridgeTo, seg.pill.theme);
      return;
    }
    const near = { left: prev ? rightEdgeOf(prev) : null, right: next ? leftEdgeOf(next) : null };
    styles[seg.key] =
      variant === 'link' ? linkSegmentStyle(seg.colors, near, seg.pill.theme) : proseSegmentStyle(seg.colors, near, seg.pill.theme);
    // A foreign end's home dot, in its half's own colour (the end's owner).
    const end = seg.key === 'from' ? fromOwner : seg.key === 'to' ? toOwner : null;
    if (foreign(end)) {
      const bridgeTo = seg.key === 'from' ? toPill.colors : null;
      styles[seg.key] = { ...styles[seg.key], ...homeDotStyle(variant, seg.colors, near, bridgeTo, seg.pill.theme) };
    }
  });
  return {
    head: styles.head ?? null,
    from: styles.from!,
    arrow: styles.arrow!,
    to: styles.to!,
    ends: { from: foreign(fromOwner), to: foreign(toOwner) },
    segments: segs.map(({ key, colors, bridgeTo }) => ({ key, colors, bridgeTo })),
    panel,
  };
}

/** The line a foreign shot end belongs to, by name, for its tooltip and words (its home dot is drawn from the pill's {@link ShotTone}). */
export interface HomeMark {
  line?: string | undefined;
}

/**
 * The pill's colours as the custom properties the `.sh-lpill` rules read. A
 * foreign station's home dot is the pill's own line's (the pill is its
 * owner's, `PillColors.home`) unless `homeDot` says otherwise (a slot's pill
 * showing another line's station, `homeDotOn`).
 */
export function pillStyle(colors: PillColors, homeDot?: PillDot | null): CSSProperties {
  const home = homeDot ?? colors.home;
  return {
    '--pill-bg': colors.bg,
    '--pill-border': colors.border,
    '--pill-bg-hover': colors.hoverBg,
    '--pill-border-hover': colors.hoverBorder,
    '--pill-text': colors.text,
    '--pill-muted': colors.muted,
    '--pill-dot': colors.dot,
    '--pill-ring': colors.ring ?? 'transparent',
    '--home-dot': home.dot,
    '--home-ring': home.ring ?? 'transparent',
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

/** The line `homeLineId` a foreign end belongs to, by name, or null for none. */
export function markOf(pill: ResolvedPill, homeLineId: number | null | undefined): HomeMark | null {
  if (homeLineId === null || homeLineId === undefined) return null;
  return { line: pill.lineNameOf?.(homeLineId) ?? undefined };
}

/**
 * A pill end's tooltip: whose station it is ("Belongs to Main", which the
 * home dot shows too, RS-LEADDOT) and that it is a loop end (no mark since
 * RS-NODOTS).
 */
export function endTitleOf(mark: HomeMark | null | undefined, loopEnd: boolean | undefined): string | undefined {
  const titles = [
    mark?.line ? i18n.t('identity.homeMarkTitle', { line: mark.line }) : null,
    loopEnd ? i18n.t('identity.loopEndMarkTitle') : null,
  ].filter((t): t is string => t !== null);
  return titles.length > 0 ? titles.join('\n') : undefined;
}

/** A shot's two station names, each with the line it belongs to when that is
 *  another line, and whether it is a loop end (said in a tooltip and in
 *  words, RS-NODOTS). */
export interface ShotEnds {
  from: string;
  to: string;
  fromMark?: HomeMark | null | undefined;
  toMark?: HomeMark | null | undefined;
  fromLoopEnd?: boolean | undefined;
  toLoopEnd?: boolean | undefined;
}

export interface PillPartsProps {
  /** Test id root: parts get `-dot`, `-id` (the context), `-loop-end` (its words),
   *  `-station-id`, `-home-dot`, `-owner` (a station's owner words), and a
   *  shot's ends `-from-end`, `-to-end` with their words `-from-owner`,
   *  `-from-loop-end`, `-to-owner`, `-to-loop-end` and home dots
   *  `-from-home-dot`, `-to-home-dot`; a two-tone shot's segments `-head`,
   *  `-from`, `-arrow`, `-to`. */
  testId?: string | undefined;
  /** Muted context before the main part, shown with a colon (`Main Line:`). */
  context?: string | null | undefined;
  /** The main part: a station name, a shot, a line name. */
  main: ReactNode;
  /** A shot pill's main part as its two ends, so each can carry its tooltip and words. Overrides `main`. */
  shotEnds?: ShotEnds | null | undefined;
  /** A two-tone shot pill's segment styles (RS-OWNERSHIP); the caller gives the
   *  pill `sh-lpill--split` and no colours of its own. Needs `shotEnds`. */
  tone?: ShotTone | null | undefined;
  /** A station pill of a loop end (RS 6): said in a tooltip and in words. */
  loopEnd?: boolean | undefined;
  /** Words read for a loop end (the station's, or a shot end's) by a screen reader. */
  loopEndLabel?: string | undefined;
  /** A station's Station ID, as a muted superscript at the end (`#42`; the `#` is not read aloud). */
  stationId?: number | null | undefined;
  /** Words read before the Station ID by a screen reader (a span pill has no aria-label). */
  stationIdLabel?: string | undefined;
  /** Words read after a station's name by a screen reader when the pill
   *  takes another line's colour (`(Main)`, RS-OWNERSHIP). A button pill's
   *  aria-label says it already. */
  ownerWords?: string | null | undefined;
  /** A foreign station (shown on a line it does not belong to): its home
   *  dot after its name, in `--home-dot` (RS-LEADDOT). A shot's ends take
   *  theirs from `tone`. */
  homeDot?: boolean | undefined;
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
  homeDot = false,
  missing = false,
  trailing,
}: PillPartsProps) {
  const part = (suffix: string) => (testId ? `${testId}-${suffix}` : undefined);
  // The leading dot in the line's colour (RS-LEADDOT), decorative: the words
  // say whose it is. A missing target has its warning glyph instead.
  const dot = missing ? (
    <span className="sh-lpill-warn" aria-hidden="true" data-testid={part('missing')}>
      &#x26A0;
    </span>
  ) : (
    <span className="sh-lpill-dot" aria-hidden="true" data-testid={part('dot')} />
  );
  // A foreign station's home dot: superscript, after its name, taking no
  // room (the `.sh-lpill-home` rules); decorative, the tooltip and the
  // words say whose station it is.
  const home = (suffix: string) => <span className="sh-lpill-home" aria-hidden="true" data-testid={part(suffix)} />;
  const ctx = context ? (
    <>
      <span className="sh-lpill-ctx" data-testid={part('id')}>
        {context}:
      </span>{' '}
    </>
  ) : null;
  const loopWords = (suffix: string) => (
    <span className="sr-only" data-testid={part(suffix)}>
      {' '}
      {loopEndLabel}
    </span>
  );
  /** A shot end's name, with whose station it is and whether it is a loop end in a tooltip and in words. */
  const end = (which: 'from' | 'to') => {
    if (!shotEnds) return null;
    const mark = which === 'from' ? shotEnds.fromMark : shotEnds.toMark;
    const loop = which === 'from' ? shotEnds.fromLoopEnd : shotEnds.toLoopEnd;
    return (
      <span className="sh-lpill-end" title={endTitleOf(mark, loop)} data-testid={part(`${which}-end`)}>
        {which === 'from' ? shotEnds.from : shotEnds.to}
        {mark?.line && (
          <span className="sr-only" data-testid={part(`${which}-owner`)}>
            {' '}
            {i18n.t('identity.homeMarkLabel', { line: mark.line })}
          </span>
        )}
        {loop && loopWords(`${which}-loop-end`)}
      </span>
    );
  };
  if (shotEnds && tone) {
    // Two-tone (RS-OWNERSHIP): the head (the dot and the context) stays in
    // the shot's colour when the from end's half is another line's; the
    // arrow is its own segment, the blend between the halves (RS-ARROW).
    return (
      <>
        {tone.head && (
          <span className="sh-lpill-seg" style={tone.head} data-testid={part('head')}>
            {dot}
            {ctx}
          </span>
        )}
        <span className="sh-lpill-seg sh-lpill-seg--to-arrow" style={tone.from} data-testid={part('from')}>
          {!tone.head && dot}
          {!tone.head && ctx}
          <span className="sh-lpill-main">{end('from')}</span>
          {tone.ends.from && home('from-home-dot')}
        </span>{' '}
        {/* The spaces between the segments are not drawn (flex items), but
            keep the pill's words those of the plain shot. */}
        <span className="sh-lpill-seg sh-lpill-seg--arrow" style={tone.arrow} data-testid={part('arrow')}>
          <span className="sh-lpill-main">{SHOT_ARROW}</span>
        </span>{' '}
        <span className="sh-lpill-seg sh-lpill-seg--from-arrow" style={tone.to} data-testid={part('to')}>
          <span className="sh-lpill-main">{end('to')}</span>
          {tone.ends.to && home('to-home-dot')}
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
            {end('from')}
            {` ${SHOT_ARROW} `}
            {end('to')}
          </>
        ) : (
          main
        )}
      </span>
      {homeDot && !shotEnds && !missing && home('home-dot')}
      {ownerWords && (
        <span className="sr-only" data-testid={part('owner')}>
          {' '}
          {ownerWords}
        </span>
      )}
      {loopEnd && loopWords('loop-end')}
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
