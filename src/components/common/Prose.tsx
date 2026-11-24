/**
 * Prose pills (RS-PROSE): a line pill inline in running text. A sentence
 * that names a station, a shot or a line ("Side / 0 is archived with its
 * comment") draws each name as a `ProsePill`: the muted context (`Side:`),
 * the bare name and the muted superscript Station ID in the line's tint, as
 * the link pills (`LinePill.tsx`) draw them; no dot or marks (RS-NODOTS),
 * the tooltip says whose station it is and a loop end. A station of another
 * line takes that line's tint and name (RS-OWNERSHIP); a shot with an end
 * of another line is two tints, blended across the arrow.
 *
 * It paints no pill shape: a bordered fill on every line of a wrapped
 * sentence stood 0 to 1 px from the one above (measured in the real app on
 * 78% of stacked pairs), so the prose pill is a soft radial tint in the
 * line's colour that fades to the panel at its edges (`prosePillColors`
 * holds its text and muted parts on both).
 *
 * It never changes the paragraph's line pitch (the `.sh-ppill` rules in
 * index.css): it is an inline box with no vertical padding, every part inside
 * has a zero line-height or a zero-height margin box, and the superscript is
 * lifted by a relative offset. It wraps across lines with each fragment
 * tinted on its own (`box-decoration-break: clone`; a two-tone shot's
 * segments tint each line's fragment once laid out, `useSegmentTints`).
 *
 * The pill's drawing is `aria-hidden` and not selectable; beside it an
 * `sr-only` span carries the part's plain words (`Side / 0`), so a screen
 * reader reads, and a copy takes, the sentence plain text writes. Below the
 * Full highlighting level the pill is that plain text, as `EntityBlob` is.
 *
 * - `<Prose text>`: a string that may hold tokens (`utils/prose.ts`), as an
 *   i18n sentence worded with `useProse()` tokens gives it;
 * - `<ProseParts parts>`: a parts list as the backend sends it;
 * - `<BackendProse text parts>`: a backend text with its optional parts.
 */
import { Fragment, useMemo, useRef, type CSSProperties, type ReactNode } from 'react';
import { useUIStore } from '../../store/uiStore';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { displayStationName, SHOT_ARROW } from '../../utils/formatters';
import { isNamePart, partOf, proseParts, type ProsePart } from '../../utils/prose';
import { withoutControls } from '../../utils/proseTokenCore';
import type { ProseShotEnd } from '../../types/ipc/ProseShotEnd';
import {
  endTitleOf,
  markOf,
  prosePillStyle,
  shotToneOf,
  useResolvePill,
  type ResolvedPill,
  type SegmentKey,
  type ShotTone,
} from './LinePill';
import { useSegmentTints } from './useSegmentTints';

type NamePart = Exclude<ProsePart, { kind: 'text' }>;

/** The arrow between a shot's ends, with no-break spaces: no line break on either side of it (RS-ARROW). */
const ARROW_GLUED = `\u00a0${SHOT_ARROW}\u00a0`;

/**
 * `text` as three runs: its first word, what lies between, its last word
 * (`['Upper', ' Sump ', 'Passage']`; one word: `['12', '', '']`), so the
 * Station ID can stay with the last word while a long name still wraps
 * between its words.
 */
export function splitWords(text: string): [string, string, string] {
  const first = /^\S*/.exec(text)?.[0] ?? '';
  if (first.length === text.length) return [text, '', ''];
  const last = /\S*$/.exec(text)?.[0] ?? '';
  return [first, text.slice(first.length, text.length - last.length), last];
}

/**
 * `text` with `before` glued to its first word and `after` to its last (no
 * line break between), the words in `textClass` when given (the muted
 * context), `before` and `after` outside it.
 */
function Glued({
  before,
  text,
  after,
  textClass,
}: {
  before?: ReactNode;
  text: string;
  after?: ReactNode;
  textClass?: string;
}) {
  const words = (w: string) => (textClass && w !== '' ? <span className={textClass}>{w}</span> : w);
  const [first, middle, last] = splitWords(text);
  if (middle === '' && last === '') {
    return (
      <span className="sh-ppill-nw">
        {before}
        {words(first)}
        {after}
      </span>
    );
  }
  return (
    <>
      <span className="sh-ppill-nw">
        {before}
        {words(first)}
      </span>
      {words(middle)}
      <span className="sh-ppill-nw">
        {words(last)}
        {after}
      </span>
    </>
  );
}

/** A station's Station ID after its name, a muted superscript (no marks since RS-NODOTS). */
function stationIdOf(stationId: number | undefined): ReactNode {
  if (stationId === undefined) return null;
  return (
    <span className="sh-ppill-sid" data-prose-sid="">
      #{stationId}
    </span>
  );
}

/** The muted context (`Side:`), then a space. */
function Head({ context }: { context?: string | undefined }) {
  if (!context) return null;
  return (
    <>
      <Glued text={`${context}:`} textClass="sh-ppill-ctx" />{' '}
    </>
  );
}

/** A shot end's tooltip: whose station it is, and whether it is a loop end (RS-NODOTS: no marks). */
function endTitle(pill: ResolvedPill, end: ProseShotEnd): string | undefined {
  return endTitleOf(markOf(pill, end.home_line), end.loop_end);
}

/** What the pill draws for `part` (aria-hidden: the sr-only words say it). */
function Drawing({
  part,
  pill,
  context,
  tone,
  tints,
}: {
  part: NamePart;
  pill: ResolvedPill;
  context: string | undefined;
  tone: ShotTone | null;
  tints: Partial<Record<SegmentKey, CSSProperties>> | null;
}) {
  if (part.kind === 'station') {
    // The whole pill is in the colour of the line the station belongs to
    // (RS-OWNERSHIP); no dot or marks (RS-NODOTS), its tooltip says whose
    // it is and whether it is a loop end.
    return (
      <>
        <Head context={context} />
        <Glued text={part.name} after={stationIdOf(part.station_id)} />
      </>
    );
  }
  if (part.kind === 'shot') {
    // A break never falls on either side of the arrow (RS-ARROW): its
    // spaces are no-break spaces, so the from end's last word, the arrow and
    // the to end's first word stay on one line (`.sh-ppill-join` holds the
    // three), while each name still wraps between its own words. Not
    // white-space: both WebKit and Chromium broke at the space before the
    // arrow inside a nowrap span when the from end's own box allowed wraps.
    if (!tone) {
      return (
        <>
          <Head context={context} />
          <span className="sh-ppill-join">
            <span className="sh-ppill-end" title={endTitle(pill, part.from)}>
              <Glued text={part.from.name} />
            </span>
            {ARROW_GLUED}
            <span className="sh-ppill-end" title={endTitle(pill, part.to)}>
              <Glued text={part.to.name} />
            </span>
          </span>
        </>
      );
    }
    // Two tints (RS-OWNERSHIP): the half holding an end of another line in
    // that line's tint; the head stays the shot's and blends into the from
    // half (RS-BLEND). The arrow is a segment of its own whose tint runs
    // from the from half's to the to half's, so the halves blend across it
    // (RS-ARROW).
    return (
      <>
        {tone.head && (
          <span className="sh-ppill-seg" style={{ ...tone.head, ...tints?.head }} data-prose-seg="head">
            <Head context={context} />
          </span>
        )}
        <span className="sh-ppill-join">
          <span
            className="sh-ppill-seg sh-ppill-seg--to-arrow"
            style={{ ...tone.from, ...tints?.from }}
            data-prose-seg="from"
            title={endTitle(pill, part.from)}
          >
            {!tone.head && <Head context={context} />}
            <Glued text={part.from.name} />
          </span>
          <span className="sh-ppill-seg sh-ppill-seg--arrow" style={{ ...tone.arrow, ...tints?.arrow }} data-prose-seg="arrow">
            {ARROW_GLUED}
          </span>
          <span
            className="sh-ppill-seg sh-ppill-seg--from-arrow"
            style={{ ...tone.to, ...tints?.to }}
            data-prose-seg="to"
            title={endTitle(pill, part.to)}
          >
            <Glued text={part.to.name} />
          </span>
        </span>
      </>
    );
  }
  return <Glued text={part.name} />;
}

/**
 * One name of a sentence as an inline pill (see the module doc). The outer
 * span carries the colours and the test hooks and paints nothing; the
 * drawing inside it is the painted pill, so the sr-only words beside it
 * (absolutely positioned) never open an empty painted fragment where the
 * pill wraps. A station of another line takes that line's tint and names it
 * as the context (RS-OWNERSHIP); a shot with an end of another line is
 * two-tone.
 */
export function ProsePill({ part: given, testId }: { part: NamePart; testId?: string | undefined }) {
  const level = useUIStore((s) => s.syntacticLevel);
  // A part that lacks a field the pill draws (a hand-built or decoded part
  // gone wrong) renders its plain words, never a crash (RS-PROSE fix 1).
  const checked = partOf(given);
  const words = withoutControls(typeof given?.text === 'string' ? given.text : '');
  if (!syntacticLevelAtLeast(level, 'full') || checked === null || checked.kind === 'text') return <>{words}</>;
  return <DrawnPill part={shown(checked)} testId={testId} />;
}

/** A {@link ProsePill} that draws its part (already checked and sanitised). */
function DrawnPill({ part, testId }: { part: NamePart; testId: string | undefined }) {
  const resolve = useResolvePill('prose');
  const owner = part.kind === 'station' && part.home_line !== undefined && part.home_line !== part.line_id ? part.home_line : null;
  const pill = resolve(owner ?? part.line_id ?? null);
  // The words drawn are the part's as it was worded: a history row keeps the
  // line names it was recorded with, and the live line only gives the colour
  // (RS-PROSE fix 2). The tooltip names the owner by the same words when the
  // part has them, else by the live line.
  const context = part.kind === 'line' ? undefined : part.context || undefined;
  const ownerName = owner === null ? null : context ?? (pill.lineNameOf?.(owner) || null);
  const tone =
    part.kind === 'shot'
      ? shotToneOf(
          resolve,
          part.line_id ?? null,
          part.from.home_line,
          part.to.home_line,
          'prose',
          { head: context ? `${context}:` : '', from: part.from.name, to: part.to.name },
          Boolean(context),
        )
      : null;
  // Each line's fragment of a two-tone shot tinted on its own (RS-BLEND).
  const vis = useRef<HTMLSpanElement>(null);
  const tints = useSegmentTints(vis, tone);
  return (
    <span
      className="sh-ppill"
      style={prosePillStyle(pill.colors, pill.theme)}
      data-prose-kind={part.kind}
      data-owner-line={owner ?? undefined}
      data-testid={testId}
      title={part.kind === 'station' ? endTitleOf(ownerName === null ? null : { line: ownerName }, part.loop_end) : undefined}
    >
      <span ref={vis} className={tone ? 'sh-ppill-vis sh-ppill-vis--split' : 'sh-ppill-vis'} aria-hidden="true">
        <Drawing part={part} pill={pill} context={context} tone={tone} tints={tints} />
      </span>
      <span className="sr-only">{withoutControls(part.text)}</span>
    </span>
  );
}

/**
 * `part` with what it draws sanitised (RS-PROSE fix 5): names and contexts
 * lose control, bidi and other invisible characters, as every displayed name
 * does (`displayStationName`), so a persisted or decoded part cannot reorder
 * or hide its own words.
 */
function shown(part: NamePart): NamePart {
  const clean = (v: string) => displayStationName(v);
  const ctx = 'context' in part && part.context !== undefined ? { context: clean(part.context) } : {};
  if (part.kind === 'shot') {
    return {
      ...part,
      ...ctx,
      from: { ...part.from, name: clean(part.from.name) },
      to: { ...part.to, name: clean(part.to.name) },
    };
  }
  if (part.kind === 'station') return { ...part, ...ctx, name: clean(part.name) };
  return { ...part, name: clean(part.name) };
}

/** A parts list: words as text, every name as a {@link ProsePill}. */
export function ProseParts({ parts, testId }: { parts: readonly ProsePart[]; testId?: string | undefined }) {
  return (
    <>
      {parts.map((part, i) =>
        isNamePart(part) ? (
          <ProsePill key={i} part={part as NamePart} testId={testId ? `${testId}-pill-${i}` : undefined} />
        ) : (
          <Fragment key={i}>{part.text}</Fragment>
        ),
      )}
    </>
  );
}

/** A string that may hold prose tokens (`utils/prose.ts`), its names as pills. */
export function Prose({ text, testId }: { text: string; testId?: string | undefined }) {
  const parts = useMemo(() => proseParts(text), [text]);
  return <ProseParts parts={parts} testId={testId} />;
}

/** A backend text: its parts when it sends some, else its plain string. */
export function BackendProse({
  text,
  parts,
  testId,
}: {
  text: string;
  parts?: readonly ProsePart[] | null | undefined;
  testId?: string | undefined;
}) {
  if (!parts || !parts.some(isNamePart)) return <>{text}</>;
  return <ProseParts parts={parts} testId={testId} />;
}
