/**
 * Prose pills (RS-PROSE): a line pill inline in running text. A sentence
 * that names a station, a shot or a line ("Side / 0 is archived with its
 * comment") draws each name as a `ProsePill`: the line's colour dot, the
 * muted context (`Side:`), the bare name, the home-line dot of a foreign
 * slot, the loop mark of a loop end and the muted superscript Station ID,
 * as the link pills (`LinePill.tsx`) draw them.
 *
 * It paints no pill shape: a bordered fill on every line of a wrapped
 * sentence stood 0 to 1 px from the one above (measured in the real app on
 * 78% of stacked pairs), so the prose pill is a soft radial tint in the
 * line's colour that fades to the panel at its edges (`prosePillColors`
 * holds its text, muted parts and dots on both).
 *
 * It never changes the paragraph's line pitch (the `.sh-ppill` rules in
 * index.css): it is an inline box with no vertical padding, every part inside
 * has a zero line-height or a zero-height margin box, and the superscript is
 * lifted by a relative offset. It wraps across lines with each fragment
 * tinted on its own (`box-decoration-break: clone`).
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
import { Fragment, useMemo, type CSSProperties, type ReactNode } from 'react';
import { useUIStore } from '../../store/uiStore';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { SHOT_ARROW } from '../../utils/formatters';
import { isNamePart, proseParts, type ProsePart } from '../../utils/prose';
import i18n from '../../i18n';
import { LoopGlyph, markOf, prosePillStyle, useProsePillColors, type HomeMark, type ResolvedPill } from './LinePill';

type NamePart = Exclude<ProsePart, { kind: 'text' }>;

/** A home-line dot after a name, with its tooltip ("Belongs to Main"). */
function ProseMark({ mark }: { mark: HomeMark }) {
  const style = { '--mark-dot': mark.dot, '--mark-ring': mark.ring ?? 'transparent' } as CSSProperties;
  return (
    <span
      className="sh-ppill-mark"
      style={style}
      title={mark.line ? i18n.t('identity.homeMarkTitle', { line: mark.line }) : undefined}
      data-prose-mark="home"
    />
  );
}

function ProseLoop() {
  return (
    <span className="sh-ppill-loop" data-prose-mark="loop">
      <LoopGlyph />
    </span>
  );
}

/**
 * `text` as three runs: its first word, what lies between, its last word
 * (`['Upper', ' Sump ', 'Passage']`; one word: `['12', '', '']`), so the
 * dot can stay with the first word and the marks with the last while a long
 * name still wraps between its words.
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

/** A name's marks: the home-line dot, the loop mark, the Station ID. */
function marksOf(
  pill: ResolvedPill,
  homeLine: number | undefined,
  loopEnd: boolean | undefined,
  stationId?: number | undefined,
): ReactNode {
  const mark = markOf(pill, homeLine);
  if (!mark && !loopEnd && stationId === undefined) return null;
  return (
    <>
      {mark && <ProseMark mark={mark} />}
      {loopEnd && <ProseLoop />}
      {stationId !== undefined && (
        <span className="sh-ppill-sid" data-prose-sid="">
          #{stationId}
        </span>
      )}
    </>
  );
}

const DOT = <span className="sh-ppill-dot" />;

/** The dot and the muted context (`Side:`), then a space; or the dot alone when there is none. */
function Head({ context }: { context?: string | undefined }) {
  if (!context) return null;
  return (
    <>
      <Glued before={DOT} text={`${context}:`} textClass="sh-ppill-ctx" />{' '}
    </>
  );
}

/** What the pill draws for `part` (aria-hidden: the sr-only words say it). */
function Drawing({ part, pill }: { part: NamePart; pill: ResolvedPill }) {
  const dot = part.kind !== 'line' && part.context ? null : DOT;
  if (part.kind === 'station') {
    return (
      <>
        <Head context={part.context} />
        <Glued before={dot} text={part.name} after={marksOf(pill, part.home_line, part.loop_end, part.station_id)} />
      </>
    );
  }
  if (part.kind === 'shot') {
    // A break may fall before the arrow, never after it.
    return (
      <>
        <Head context={part.context} />
        <Glued before={dot} text={part.from.name} after={marksOf(pill, part.from.home_line, part.from.loop_end)} />{' '}
        <Glued before={`${SHOT_ARROW} `} text={part.to.name} after={marksOf(pill, part.to.home_line, part.to.loop_end)} />
      </>
    );
  }
  return <Glued before={DOT} text={part.name} />;
}

/**
 * One name of a sentence as an inline pill (see the module doc). The outer
 * span carries the colours and the test hooks and paints nothing; the
 * drawing inside it is the painted pill, so the sr-only words beside it
 * (absolutely positioned) never open an empty painted fragment where the
 * pill wraps.
 */
export function ProsePill({ part, testId }: { part: NamePart; testId?: string | undefined }) {
  const level = useUIStore((s) => s.syntacticLevel);
  const pill = useProsePillColors(part.line_id ?? null);
  if (!syntacticLevelAtLeast(level, 'full')) return <>{part.text}</>;
  return (
    <span className="sh-ppill" style={prosePillStyle(pill.colors, pill.theme)} data-prose-kind={part.kind} data-testid={testId}>
      <span className="sh-ppill-vis" aria-hidden="true">
        <Drawing part={part} pill={pill} />
      </span>
      <span className="sr-only">{part.text}</span>
    </span>
  );
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
