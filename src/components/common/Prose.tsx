/**
 * Prose pills (RS-PROSE): the line pill inline in running text. A sentence
 * that names a station, a shot or a line ("Side / 0 is archived with its
 * comment") draws each name as a `ProsePill`: the line's colour dot, the
 * muted context (`Side:`), the bare name, the home-line dot of a foreign
 * slot, the loop mark of a loop end and the muted superscript Station ID,
 * in the colours of `theme/pillColors.ts`, as the link pills (`LinePill.tsx`).
 *
 * It never changes the paragraph's line pitch (the `.sh-ppill` rules in
 * index.css): it is an inline box with no vertical padding, every part inside
 * has a zero line-height or a zero-height margin box, and the superscript is
 * lifted by a relative offset. It wraps across lines with each fragment
 * rounded (`box-decoration-break: clone`).
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
import { Fragment, useMemo, type CSSProperties } from 'react';
import { useUIStore } from '../../store/uiStore';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { SHOT_ARROW } from '../../utils/formatters';
import { isNamePart, proseParts, type ProsePart } from '../../utils/prose';
import type { ProseShotEnd } from '../../types/ipc/ProseShotEnd';
import i18n from '../../i18n';
import { LoopGlyph, markOf, pillStyle, usePillColors, type HomeMark, type ResolvedPill } from './LinePill';

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

/** A name with its marks, kept on one line. */
function NameWithMarks({
  name,
  pill,
  homeLine,
  loopEnd,
  stationId,
}: {
  name: string;
  pill: ResolvedPill;
  homeLine?: number | undefined;
  loopEnd?: boolean | undefined;
  stationId?: number | undefined;
}) {
  const mark = markOf(pill, homeLine);
  return (
    <span className="sh-ppill-name">
      {name}
      {mark && <ProseMark mark={mark} />}
      {loopEnd && <ProseLoop />}
      {stationId !== undefined && (
        <span className="sh-ppill-sid" data-prose-sid="">
          #{stationId}
        </span>
      )}
    </span>
  );
}

function ShotEnd({ end, pill }: { end: ProseShotEnd; pill: ResolvedPill }) {
  return <NameWithMarks name={end.name} pill={pill} homeLine={end.home_line} loopEnd={end.loop_end} />;
}

/** The dot and the muted context (`Side:`), kept together. */
function Head({ context }: { context?: string | undefined }) {
  return (
    <span className="sh-ppill-head">
      <span className="sh-ppill-dot" />
      {context ? <span className="sh-ppill-ctx">{context}:</span> : null}
    </span>
  );
}

/** One name of a sentence as an inline pill (see the module doc). */
export function ProsePill({ part, testId }: { part: NamePart; testId?: string | undefined }) {
  const level = useUIStore((s) => s.syntacticLevel);
  const pill = usePillColors(part.line_id ?? null);
  if (!syntacticLevelAtLeast(level, 'full')) return <>{part.text}</>;
  let drawing;
  if (part.kind === 'station') {
    drawing = (
      <>
        <Head context={part.context} />{part.context ? ' ' : null}
        <NameWithMarks
          name={part.name}
          pill={pill}
          homeLine={part.home_line}
          loopEnd={part.loop_end}
          stationId={part.station_id}
        />
      </>
    );
  } else if (part.kind === 'shot') {
    drawing = (
      <>
        <Head context={part.context} />{part.context ? ' ' : null}
        <ShotEnd end={part.from} pill={pill} />
        {` ${SHOT_ARROW} `}
        <ShotEnd end={part.to} pill={pill} />
      </>
    );
  } else {
    drawing = (
      <>
        <Head />
        {part.name}
      </>
    );
  }
  return (
    <span
      className="sh-ppill"
      style={pillStyle(pill.colors)}
      data-prose-kind={part.kind}
      data-testid={testId}
    >
      <span className="sh-ppill-vis" aria-hidden="true">
        {drawing}
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
