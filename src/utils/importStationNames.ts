/**
 * The names the stations of a line being imported will get (SPEC 4.1/4.2).
 *
 * An import wizard shows a station of an incoming line by the name it will
 * have once imported, never by its position. A DMP row carries its MNemo
 * station number, which the import sends as that station's name
 * (`stationNamesForEmittedShots`). The backend names the rest by the creation
 * rules (`fill_line_station_names`: End, Mid and Prepend), seeds a line with
 * no names at all from the station indexes, and makes every name unique in its
 * line (`settle_line_station_names`). This is a port of those rules from
 * `src-tauri/crates/models/src/project/station_naming.rs`, so the wizard shows
 * exactly the name the import gives; keep the two in step.
 */
import type { EditableShot } from '../components/common/editableShot';
import type { CrossLineSuggestion } from '../types/commands';
import { mnemoStationNumber } from '../hooks/useImportState';
import { formatStationLabel } from './formatters';
import { stationNamesForEmittedShots } from './importLineConfig';
import { lineDisplayName, type IdentityLabels } from './stationIdentity';

type Slots = Array<string | null>;

const U64_MAX = 2n ** 64n - 1n;

/**
 * A name split into its prefix and trailing decimal number, with the digit
 * count (`A3` -> A, 3, 1; `007` -> '', 7, 3). Null when the name does not end
 * in a digit or the number does not fit a u64 (the backend's limit).
 */
function trailingNumber(name: string): { prefix: string; number: bigint; width: number } | null {
  let split = name.length;
  while (split > 0 && name.charCodeAt(split - 1) >= 48 && name.charCodeAt(split - 1) <= 57) split--;
  const width = name.length - split;
  if (width === 0) return null;
  const number = BigInt(name.slice(split));
  return number > U64_MAX ? null : { prefix: name.slice(0, split), number, width };
}

const numbered = (prefix: string, number: bigint, width: number): string =>
  `${prefix}${number.toString().padStart(width, '0')}`;

/** `prefix` + the first number above `from` whose name is free. */
function nextNumbered(prefix: string, from: bigint, width: number, taken: ReadonlySet<string>): string {
  let number = from;
  for (;;) {
    if (number < U64_MAX) number += 1n;
    const candidate = numbered(prefix, number, width);
    if (!taken.has(candidate) || number === U64_MAX) return candidate;
  }
}

/** The highest number of `prefix`'s family among `names`, or null. */
function familyMax(prefix: string, names: Iterable<string>): bigint | null {
  let max: bigint | null = null;
  for (const name of names) {
    const split = trailingNumber(name);
    if (split && split.prefix === prefix && (max === null || split.number > max)) max = split.number;
  }
  return max;
}

/** One above the highest all-digit name (`0` when there is none), while taken. */
function nextFreeInteger(taken: ReadonlySet<string>): string {
  const max = familyMax('', taken);
  return max === null ? '0' : nextNumbered('', max, 1, taken);
}

/** End: the station after `previous` - its number plus one, while taken. */
function nameAfterEnd(previous: string, taken: ReadonlySet<string>): string {
  const split = trailingNumber(previous);
  return split ? nextNumbered(split.prefix, split.number, split.width, taken) : nextFreeInteger(taken);
}

/** Prepend: the station before `first` - the next free number of its family. */
function nameBeforeStart(first: string, taken: ReadonlySet<string>): string {
  const split = trailingNumber(first);
  if (!split) return nextFreeInteger(taken);
  const familyTop = familyMax(split.prefix, taken) ?? split.number;
  const max = familyTop > split.number ? familyTop : split.number;
  return nextNumbered(split.prefix, max, split.width, taken);
}

/** `a`..`z`, then `aa`..`zz`, `aaa`, ... (bijective base 26). */
function letterSuffix(index: number): string {
  let n = index + 1;
  let out = '';
  while (n > 0) {
    n -= 1;
    out = String.fromCharCode(97 + (n % 26)) + out;
    n = Math.floor(n / 26);
  }
  return out;
}

/** Mid (and a repeat or taken seed): `base` + the first free lowercase suffix. */
function nameWithFreeLetter(base: string, taken: ReadonlySet<string>): string {
  for (let index = 0; ; index++) {
    const candidate = `${base}${letterSuffix(index)}`;
    if (!taken.has(candidate)) return candidate;
  }
}

const isNamed = (slot: string | null | undefined): slot is string => slot != null && slot.trim() !== '';

/**
 * Fill the gaps of a partly named line in place, station order: after the
 * last named station by End, between two named stations by Mid suffixes of
 * the one before, before the first named station by Prepend going backwards.
 * A line with no name at all is left for {@link settleLineStationNames}.
 */
export function fillLineStationNames(names: Slots): void {
  const firstNamed = names.findIndex(isNamed);
  if (firstNamed < 0) return;
  let lastNamed = firstNamed;
  names.forEach((slot, i) => {
    if (isNamed(slot)) lastNamed = i;
  });
  const taken = new Set(names.filter((n): n is string => n !== null));
  let anchor: string | null = null;
  for (let i = firstNamed; i < names.length; i++) {
    const slot = names[i];
    if (isNamed(slot)) {
      anchor = slot;
      continue;
    }
    if (anchor === null) continue;
    const fresh = i > lastNamed ? nameAfterEnd(names[i - 1] ?? anchor, taken) : nameWithFreeLetter(anchor, taken);
    taken.add(fresh);
    names[i] = fresh;
  }
  for (let i = firstNamed - 1; i >= 0; i--) {
    const fresh = nameBeforeStart(names[i + 1] ?? '', taken);
    taken.add(fresh);
    names[i] = fresh;
  }
}

/**
 * Make a line's names unique in place, station order: the first holder keeps a
 * name, a later one gets its Mid suffix (`5`, `5` -> `5`, `5a`), and a missing
 * name is seeded from the station's index (letter-suffixed when taken).
 */
export function settleLineStationNames(names: Slots): void {
  const taken = new Set(names.filter((n): n is string => n !== null));
  const kept = new Set<string>();
  names.forEach((slot, i) => {
    let fresh: string;
    if (isNamed(slot) && !kept.has(slot)) {
      fresh = slot;
    } else if (isNamed(slot)) {
      fresh = nameWithFreeLetter(slot, taken);
    } else {
      const seed = String(i);
      fresh = taken.has(seed) ? nameWithFreeLetter(seed, taken) : seed;
    }
    taken.add(fresh);
    kept.add(fresh);
    names[i] = fresh;
  });
}

/**
 * The names the stations of an incoming line get, by station index in the
 * order of `rows` (the file's order, before any reverse). `rows` are the rows
 * the wizard will send; `reverse` whether the import reverses the line, since
 * the backend fills and settles the names in the line's final order.
 */
export function importLineStationNames(rows: readonly EditableShot[], reverse = false): string[] {
  const slots: Slots = Array.from({ length: rows.length + 1 }, () => null);
  for (const [index, name] of stationNamesForEmittedShots(rows)) {
    if (index < slots.length) slots[index] = name;
  }
  if (reverse) slots.reverse();
  // The backend fills only a line the import names some stations of.
  if (slots.some((slot) => slot !== null)) fillLineStationNames(slots);
  settleLineStationNames(slots);
  if (reverse) slots.reverse();
  return slots.map((slot) => slot ?? '');
}

/**
 * The name of station `index` of a DMP line, counted in the file's own
 * numbering (the importable shots before any wizard edit - the space the
 * wizard's suggestions address): its MNemo station number. Station 0 is the
 * line start; station k ends the k-th importable shot.
 */
export function dmpFileStationName(index: number): string {
  return String(index === 0 ? mnemoStationNumber(0) - 1 : mnemoStationNumber(index - 1));
}

/** `Line A / 5`: station `index` (file numbering) of the incoming DMP line `lineName`. */
export function dmpFileStationLabel(index: number, lineName: string | null | undefined): string {
  return formatStationLabel(dmpFileStationName(index), lineDisplayName(lineName));
}

/**
 * `Main Line / 17`: station `index` at the far end of a DMP suggestion - a
 * station of an existing project line, named by the project, or of another
 * incoming line, in that line's file numbering.
 */
export function suggestionTargetStationLabel(
  labels: Pick<IdentityLabels, 'stationName'>,
  suggestion: Pick<CrossLineSuggestion, 'targetType' | 'existingLineId' | 'existingLineName'>,
  index: number,
): string {
  const name =
    suggestion.targetType === 'existing' && suggestion.existingLineId != null
      ? labels.stationName({ line_id: suggestion.existingLineId, station_index: index })
      : dmpFileStationName(index);
  return formatStationLabel(name, lineDisplayName(suggestion.existingLineName));
}
