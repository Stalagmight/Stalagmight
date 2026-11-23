/**
 * Prose that names stations, shots and lines (RS-PROSE): running text draws
 * each name as an inline prose pill (`ProsePill`, `components/common/Prose.tsx`)
 * in place of the plain label ("Side / 0 is archived" reads [Side: 0 #29] is
 * archived).
 *
 * A name is a {@link ProsePart}, the shape the backend sends beside a text it
 * words (`models::prose`): the words plain text has in its place (`text`,
 * "Side / 12 (Main)") and what the pill draws (the line's colour, the muted
 * context, the bare name, the home-line dot, the loop mark, the Station ID).
 *
 * A sentence the frontend words through i18n puts a **token** where the name
 * goes: the part, URI-encoded JSON between U+0001 and U+0002, which no
 * displayed name holds (`displayStationName` drops C0 controls). A token is a
 * plain string, so it passes through `t()` interpolation, `listNames` and the
 * toast store unchanged, and `<Prose text=...>` splits the result back into
 * words and pills. Plain-text contexts (aria-labels, tooltips, clipboard,
 * logs) keep the plain label; {@link proseToPlain} gives it for a string that
 * already holds tokens.
 */
import type { ProsePart } from '../types/ipc/ProsePart';
import type { ProseStation } from '../types/ipc/ProseStation';
import type { StationIdentity } from '../types/ipc/StationIdentity';
import type { StationRef } from '../types/project';
import { displayStationName, formatStationLabel } from './formatters';
import {
  lineNameOf,
  planStationLabelOf,
  shotIndexOf,
  shotLabelAt,
  shownSlotOf,
  shownStationIdOf,
  stationLabelOf,
  stationNameOf,
  type IdentitySource,
} from './stationIdentity';

export type { ProsePart };

const OPEN = '\u0001';
const CLOSE = '\u0002';
/** One token: the opening marker, the encoded part, the closing marker. */
const TOKEN = new RegExp(`${OPEN}([^${OPEN}${CLOSE}]*)${CLOSE}`, 'g');
/** A marker left over from a cut token. */
const STRAY = new RegExp(`[${OPEN}${CLOSE}]`, 'g');

/** A part's plain words. */
export function partText(part: ProsePart): string {
  return part.text;
}

/** Whether a part names something (anything but words). */
export function isNamePart(part: ProsePart): boolean {
  return part.kind !== 'text';
}

/** `part` as a token for an i18n sentence (see the module doc). */
export function proseToken(part: ProsePart): string {
  if (part.kind === 'text') return part.text;
  return `${OPEN}${encodeURIComponent(JSON.stringify(part))}${CLOSE}`;
}

function decode(payload: string): ProsePart | null {
  try {
    const part = JSON.parse(decodeURIComponent(payload)) as ProsePart;
    return part && typeof part === 'object' && typeof part.text === 'string' && typeof part.kind === 'string'
      ? part
      : null;
  } catch {
    return null;
  }
}

/**
 * The parts of a string that may hold tokens: its words and each named part,
 * adjacent words joined. A malformed token reads as nothing (its markers are
 * never shown).
 */
export function proseParts(text: string): ProsePart[] {
  const parts: ProsePart[] = [];
  let words = '';
  let last = 0;
  for (const match of text.matchAll(TOKEN)) {
    words += text.slice(last, match.index);
    last = match.index + match[0].length;
    const part = decode(match[1] ?? '');
    if (part === null) continue;
    if (part.kind === 'text') {
      words += part.text;
      continue;
    }
    if (words !== '') parts.push({ kind: 'text', text: words });
    words = '';
    parts.push(part);
  }
  words += text.slice(last);
  // A stray marker (a cut token) never reaches the screen.
  words = words.replace(STRAY, '');
  if (words !== '') parts.push({ kind: 'text', text: words });
  return parts;
}

/** Whether a string holds a token. */
export function hasProse(text: string): boolean {
  return text.includes(OPEN);
}

/** The plain string of a string that may hold tokens: each token by its words. */
export function proseToPlain(text: string): string {
  if (!hasProse(text)) return text;
  return proseParts(text).map(partText).join('');
}

/** A backend parts list as a token string (words stay words). */
export function partsToProse(parts: readonly ProsePart[]): string {
  return parts.map(proseToken).join('');
}

/** The plain string of a parts list (what the backend sends beside it). */
export function partsToPlain(parts: readonly ProsePart[]): string {
  return parts.map(partText).join('');
}

/**
 * A backend text as a token string: its parts when it has some (`*_parts`
 * fields; the parts carry the same words), else the plain string.
 */
export function backendProse(plain: string, parts: readonly ProsePart[] | null | undefined): string {
  return parts && parts.some(isNamePart) ? partsToProse(parts) : plain;
}

// ----- parts the frontend words ------------------------------------------------

/**
 * The slot `ref` as a station part: the words of `stationLabelOf` (with or
 * without the line), and what `StationBlob` draws: the slot's line colour,
 * the line as the context with `withLine`, the bare name, the home-line dot
 * of a foreign slot, the loop mark of a loop end and the Station ID it shows.
 */
export function stationPartOf(src: IdentitySource, ref: StationRef, withLine = false): ProsePart {
  const shown = shownSlotOf(src, ref);
  const home = shown?.foreign && shown.home_line !== undefined && shown.home_line !== ref.line_id ? shown.home_line : undefined;
  const station: ProseStation = {
    text: stationLabelOf(src, ref, withLine),
    line_id: ref.line_id,
    name: formatStationLabel(stationNameOf(src, ref)),
    slot: ref,
  };
  if (withLine) station.context = lineNameOf(src, ref.line_id);
  if (home !== undefined) station.home_line = home;
  if (shown?.loop_end) station.loop_end = true;
  const id = shownStationIdOf(src, ref);
  if (id !== null) station.station_id = id;
  return { kind: 'station', ...station };
}

/** A Plan pin as a station part: the words of `planStationLabelOf`, drawn as the slot's pill with its line. */
export function planStationPartOf(src: IdentitySource, ref: StationRef): ProsePart {
  const part = stationPartOf(src, ref, true);
  return { ...part, text: planStationLabelOf(src, ref) };
}

/**
 * An identity a preview names (a record, a hidden closure end) as a station
 * part: `Side / 0` by its home line (`identityLabelOf`), coloured by that
 * line, with its Station ID. Without a home line its name alone, neutral.
 */
export function identityPartOf(
  identity: StationIdentity,
  lineName: (lineId: number) => string,
  withLine = true,
): ProsePart {
  const home = identity.home_line;
  const line = withLine && home !== undefined ? lineName(home) : null;
  const station: ProseStation = {
    text: formatStationLabel(identity.name, line),
    name: displayStationName(identity.name),
    station_id: identity.id,
  };
  if (home !== undefined) station.line_id = home;
  if (line !== null) station.context = line;
  return { kind: 'station', ...station };
}

/** Line `lineId` as a line part: its display name, `"Main"` with `quoted`. */
export function linePartOf(src: IdentitySource, lineId: number, quoted = false): ProsePart {
  const name = lineNameOf(src, lineId);
  return { kind: 'line', text: quoted ? `"${name}"` : name, line_id: lineId, name };
}

/** A line no longer in the project (or never in it) by its name: a neutral line part. */
export function namedLinePart(name: string, lineId?: number | null): ProsePart {
  const part: ProsePart = { kind: 'line', text: name, name };
  if (lineId !== null && lineId !== undefined) part.line_id = lineId;
  return part;
}

/**
 * The shot at 0-based `shotIndex` of line `lineId` as a shot part: the words
 * of `shotLabelAt`, each end's bare name with its home-line dot and loop mark.
 */
export function shotPartAt(src: IdentitySource, lineId: number, shotIndex: number, withLine = false): ProsePart {
  const end = (index: number) => {
    const ref = { line_id: lineId, station_index: index };
    const shown = shownSlotOf(src, ref);
    const out: { name: string; home_line?: number; loop_end?: boolean } = {
      name: formatStationLabel(stationNameOf(src, ref)),
    };
    if (shown?.foreign && shown.home_line !== undefined && shown.home_line !== lineId) out.home_line = shown.home_line;
    if (shown?.loop_end) out.loop_end = true;
    return out;
  };
  const part: ProsePart = {
    kind: 'shot',
    text: shotLabelAt(src, lineId, shotIndex, withLine),
    line_id: lineId,
    from: end(shotIndex),
    to: end(shotIndex + 1),
  };
  if (withLine) part.context = lineNameOf(src, lineId);
  return part;
}

/** {@link shotPartAt} for a shot addressed by its id; null when the shot is unknown. */
export function shotPartOf(src: IdentitySource, lineId: number, shotId: number, withLine = false): ProsePart | null {
  const k = shotIndexOf(src, lineId, shotId);
  return k === null ? null : shotPartAt(src, lineId, k, withLine);
}

/**
 * Token builders bound to one identity source, mirroring the plain labels
 * (`labels.station` -> `prose.station`): what {@link useProse} returns.
 */
export interface ProseTokens {
  /** A slot, as `labels.station(ref, withLine)` words it. */
  station: (ref: StationRef, withLine?: boolean) => string;
  /** A Plan pin, as `labels.planStation(ref)`. */
  planStation: (ref: StationRef) => string;
  /** An identity by its home line, as `identityLabelOf`. */
  identity: (identity: StationIdentity, withLine?: boolean) => string;
  /** A line by its name, `"Main"` with `quoted`. */
  line: (lineId: number, quoted?: boolean) => string;
  /** A shot, as `labels.shotAt(lineId, shotIndex, withLine)`. */
  shotAt: (lineId: number, shotIndex: number, withLine?: boolean) => string;
  /** A shot by its id, as `labels.shot(lineId, shotId, withLine)`; null when unknown. */
  shot: (lineId: number, shotId: number, withLine?: boolean) => string | null;
  /** Any part. */
  part: (part: ProsePart) => string;
}

export function proseTokens(src: IdentitySource): ProseTokens {
  const lineName = (lineId: number) => lineNameOf(src, lineId);
  return {
    station: (ref, withLine) => proseToken(stationPartOf(src, ref, withLine)),
    planStation: (ref) => proseToken(planStationPartOf(src, ref)),
    identity: (identity, withLine) => proseToken(identityPartOf(identity, lineName, withLine)),
    line: (lineId, quoted) => proseToken(linePartOf(src, lineId, quoted)),
    shotAt: (lineId, shotIndex, withLine) => proseToken(shotPartAt(src, lineId, shotIndex, withLine)),
    shot: (lineId, shotId, withLine) => {
      const part = shotPartOf(src, lineId, shotId, withLine);
      return part === null ? null : proseToken(part);
    },
    part: proseToken,
  };
}
