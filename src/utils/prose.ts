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
 * goes: the part, URI-encoded JSON between U+0001 and U+0002, after this page
 * load's random nonce (`proseTokenCore.ts`). A value a user or a file
 * controls can neither know the nonce nor hold a marker: every `t()`
 * interpolation and every value a sentence is built from by hand goes
 * through `proseValue`, which drops the markers and other C0 controls, and a
 * token is only read when it has a part's full shape (`partOf`). A token is a
 * plain string, so it passes through `t()` interpolation, `listNames` and the
 * toast store unchanged, and `<Prose text=...>` splits the result back into
 * words and pills. Plain-text contexts (aria-labels, tooltips, clipboard,
 * logs) keep the plain label; {@link proseToPlain} gives it for a string that
 * already holds tokens.
 */
import type { ProsePart } from '../types/ipc/ProsePart';
import type { ProseStation } from '../types/ipc/ProseStation';
import type { ProseShotEnd } from '../types/ipc/ProseShotEnd';
import { CLOSE, NONCE, OPEN, TOKEN, ownPayload, proseValue, withoutControls } from './proseTokenCore';
import type { StationIdentity } from '../types/ipc/StationIdentity';
import type { StationRef } from '../types/project';
import { displayStationName, formatStationLabel } from './formatters';
import {
  lineNameOf,
  ownerLineOf,
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
export { proseValue };

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
  if (part.kind === 'text') return proseValue(part.text);
  return `${OPEN}${NONCE}:${encodeURIComponent(JSON.stringify(part))}${CLOSE}`;
}

const isString = (v: unknown): v is string => typeof v === 'string';
const isInt = (v: unknown): v is number => typeof v === 'number' && Number.isInteger(v);
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/** The optional fields of a part, copied only when they have their type. */
function optional(src: Record<string, unknown>, keys: Record<string, (v: unknown) => boolean>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, ok] of Object.entries(keys)) if (src[key] !== undefined && ok(src[key])) out[key] = src[key];
  return out;
}

function shotEnd(v: unknown): ProseShotEnd | null {
  if (!isObject(v) || !isString(v.name)) return null;
  return { name: v.name, ...optional(v, { home_line: isInt, loop_end: (x) => typeof x === 'boolean' }) } as ProseShotEnd;
}

/**
 * `value` as a part when it has a part's full shape (RS-PROSE fix 1): its
 * kind, its words, and per kind the fields the pill draws (a station's and a
 * line's name, a shot's two ends with their names), each of its type; the
 * known optional fields are kept only when they have theirs. Anything else
 * (a missing name, an unknown kind) is null, and the token reads as words.
 */
export function partOf(value: unknown): ProsePart | null {
  if (!isObject(value) || !isString(value.text)) return null;
  const text = value.text;
  switch (value.kind) {
    case 'text':
      return { kind: 'text', text };
    case 'station': {
      if (!isString(value.name)) return null;
      const slot = isObject(value.slot) && isInt(value.slot.line_id) && isInt(value.slot.station_index)
        ? { slot: { line_id: value.slot.line_id, station_index: value.slot.station_index } }
        : {};
      return {
        kind: 'station',
        text,
        name: value.name,
        ...optional(value, { line_id: isInt, context: isString, home_line: isInt, loop_end: (x) => typeof x === 'boolean', station_id: isInt }),
        ...slot,
      } as ProsePart;
    }
    case 'shot': {
      const from = shotEnd(value.from);
      const to = shotEnd(value.to);
      if (!from || !to) return null;
      return { kind: 'shot', text, from, to, ...optional(value, { line_id: isInt, context: isString }) } as ProsePart;
    }
    case 'line':
      if (!isString(value.name)) return null;
      return { kind: 'line', text, name: value.name, ...optional(value, { line_id: isInt }) } as ProsePart;
    default:
      return null;
  }
}

function decode(payload: string): ProsePart | null {
  const own = ownPayload(payload);
  if (own === null) return null;
  try {
    return partOf(JSON.parse(decodeURIComponent(own)));
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
    // Not one of this page's tokens, or not a part's shape: its words, never a pill.
    if (part === null) {
      words += ownPayload(match[1] ?? '') === null ? (match[1] ?? '') : '';
      continue;
    }
    if (part.kind === 'text') {
      words += part.text;
      continue;
    }
    if (words !== '') parts.push({ kind: 'text', text: words });
    words = '';
    parts.push(part);
  }
  words += text.slice(last);
  // A stray marker (a cut token) or another control never reaches the screen.
  words = withoutControls(words);
  if (words !== '') parts.push({ kind: 'text', text: words });
  return parts;
}

/** Whether a string holds a token. */
export function hasProse(text: string): boolean {
  return text.includes(OPEN);
}

/** The plain string of a string that may hold tokens: each token by its words. */
export function proseToPlain(text: string): string {
  if (!hasProse(text)) return withoutControls(text);
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
  return parts && parts.some(isNamePart) ? partsToProse(parts) : proseValue(plain);
}

// ----- parts the frontend words ------------------------------------------------

/**
 * The slot `ref` as a station part: the words of `stationLabelOf` (with or
 * without the line), and what `StationBlob` draws: the colour of the line
 * the station belongs to (the slot's, or for a foreign slot or a loop end
 * of another line `ownerLineOf`'s, RS-OWNERSHIP), with `withLine` that line
 * as the context, the bare name, the loop mark of a loop end and the
 * Station ID it shows.
 */
export function stationPartOf(src: IdentitySource, ref: StationRef, withLine = false): ProsePart {
  const shown = shownSlotOf(src, ref);
  const home = ownerLineOf(src, ref);
  const station: ProseStation = {
    text: stationLabelOf(src, ref, withLine),
    line_id: ref.line_id,
    name: formatStationLabel(stationNameOf(src, ref)),
    slot: ref,
  };
  // The context names the line the pill is coloured by (RS-OWNERSHIP).
  if (withLine) station.context = lineNameOf(src, home ?? ref.line_id);
  if (home !== null) station.home_line = home;
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
    const owner = ownerLineOf(src, ref);
    if (owner !== null) out.home_line = owner;
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
