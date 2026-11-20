/**
 * Client-side checks for the names the backend requires (SPEC 4.1/4.2): a line
 * name is required and project-unique, a station name is required and unique
 * within its line. The backend refuses the same cases (`validate_line_name`,
 * `validate_station_name`: trimmed, exact match); these let a dialog say so
 * inline and disable its submit before the round trip. A backend refusal is
 * still surfaced, because another window or an undo can change the names.
 */
import i18n from '../i18n';

/** Why a name cannot be used: none given, a character no name may hold, or
 *  another line / station has it. */
export type NameProblem = 'empty' | 'invalidChars' | 'duplicate';

/**
 * Characters no line, station or Section name may hold (the backend refuses
 * them too): the control characters (Cc) and the invisible format characters
 * that can make a name read as something it is not (Trojan Source) - the
 * left-to-right / right-to-left marks, the bidi embeddings, overrides and
 * isolates, the zero-width space and joiners, and the BOM.
 */
const FORBIDDEN_NAME_CHARS = /[\u0000-\u001F\u007F-\u009F\u200B-\u200F\u202A-\u202E\u2066-\u2069\uFEFF]/;

/**
 * Whether `name` holds a character no name may hold, once trimmed (a pasted
 * trailing newline is trimmed away, not refused). A BOM counts anywhere: the
 * JS trim strips one at the edges, the backend's does not.
 */
export function hasForbiddenNameChars(name: string): boolean {
  return FORBIDDEN_NAME_CHARS.test(name.trim()) || name.includes('\uFEFF');
}

/**
 * Whether `name` can name a line, given the project's `lines`. `exceptLineId`
 * is the line being renamed (its own name is not a clash). Trimmed, exact.
 */
export function lineNameProblem(
  name: string,
  lines: ReadonlyArray<{ id: number; name: string | null }> | null | undefined,
  exceptLineId?: number | null,
): NameProblem | null {
  const trimmed = name.trim();
  if (trimmed === '') return 'empty';
  if (hasForbiddenNameChars(name)) return 'invalidChars';
  const taken = (lines ?? []).some((l) => l.id !== exceptLineId && (l.name ?? '').trim() === trimmed);
  return taken ? 'duplicate' : null;
}

/**
 * Whether `name` can rename a station, given the line it would collide on
 * (`takenOnLine`, from `stationNameTakenOn`: a junction's name must be free on
 * every line through it, plan 5.3). Trimmed, exact.
 */
export function stationRenameProblem(name: string, takenOnLine: number | null): NameProblem | null {
  const trimmed = name.trim();
  if (trimmed === '') return 'empty';
  if (hasForbiddenNameChars(name)) return 'invalidChars';
  return takenOnLine === null ? null : 'duplicate';
}

/** The inline message for a station-name problem; `lineName` names the
 *  station's line, `name` is the name typed. */
export function stationNameProblemMessage(problem: NameProblem, lineName: string, name: string): string {
  switch (problem) {
    case 'empty':
      return i18n.t('identity.stationNameEmpty');
    case 'invalidChars':
      return i18n.t('identity.nameInvalidChars');
    case 'duplicate':
      return i18n.t('identity.stationNameTaken', { line: lineName, name: name.trim() });
  }
}

function takenNames(lines: ReadonlyArray<{ name: string | null }> | null | undefined): Set<string> {
  return new Set((lines ?? []).map((l) => (l.name ?? '').trim()).filter((n) => n !== ''));
}

/** The first `Line N` (N = 1, 2, ...) no line of the project holds. */
export function nextFreeLineName(lines: ReadonlyArray<{ name: string | null }> | null | undefined): string {
  const taken = takenNames(lines);
  for (let n = 1; ; n++) {
    const candidate = `Line ${n}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/**
 * `base` if no line holds it (and it is not in `alsoTaken`), else `base (2)`,
 * `base (3)`, ... - the rule the backend applies on its automatic paths.
 */
export function uniqueLineName(
  base: string,
  lines: ReadonlyArray<{ name: string | null }> | null | undefined,
  alsoTaken: Iterable<string> = [],
): string {
  const taken = takenNames(lines);
  for (const n of alsoTaken) taken.add(n.trim());
  const trimmed = base.trim();
  if (trimmed !== '' && !taken.has(trimmed)) return trimmed;
  for (let n = 2; ; n++) {
    const candidate = `${trimmed} (${n})`;
    if (!taken.has(candidate)) return candidate;
  }
}

/** A problem with one of the two names a split gives its halves. */
export type SplitNameProblem = NameProblem | 'same';

type NamedLines = ReadonlyArray<{ id: number; name: string | null }> | null | undefined;

/**
 * The names a split of line `lineId` (named `lineName`) offers its halves:
 * `<name> A` and `<name> B`, each made unique against the other lines and
 * against each other. Half A keeps the line, so the line's own name is free.
 */
export function defaultSplitNames(
  lineName: string | null | undefined,
  lines: NamedLines,
  lineId: number | null | undefined,
): { a: string; b: string } {
  const others = (lines ?? []).filter((l) => l.id !== lineId);
  const base = (lineName ?? '').trim() || 'Line';
  const a = uniqueLineName(`${base} A`, others);
  const b = uniqueLineName(`${base} B`, others, [a]);
  return { a, b };
}

/**
 * What is wrong with the names typed for a split's two halves, per field.
 * Each must be a usable line name (the split line excluded: half A keeps it
 * and its old name goes), and the two must differ (said on B).
 */
export function splitNameProblems(
  nameA: string,
  nameB: string,
  lines: NamedLines,
  lineId: number | null | undefined,
): { a: SplitNameProblem | null; b: SplitNameProblem | null } {
  const a = lineNameProblem(nameA, lines, lineId);
  const b = lineNameProblem(nameB, lines, lineId);
  const same = a === null && b === null && nameA.trim() === nameB.trim();
  return { a, b: same ? 'same' : b };
}

/** The inline message for a line-name problem; `name` is the name typed. */
export function lineNameProblemMessage(problem: SplitNameProblem, name: string): string {
  switch (problem) {
    case 'empty':
      return i18n.t('identity.lineNameEmpty');
    case 'invalidChars':
      return i18n.t('identity.nameInvalidChars');
    case 'duplicate':
      return i18n.t('identity.lineNameTaken', { name: name.trim() });
    case 'same':
      return i18n.t('identity.lineNamesSame');
  }
}

/**
 * Whether a backend error is its refusal of a line name (`validate_line_name`:
 * "A line name cannot be empty", `A line named "X" already exists`, "A line
 * name cannot contain control or invisible formatting characters"), so the
 * dialog shows it at the name field rather than as a general failure.
 */
export function isLineNameRefusal(error: string | null | undefined): boolean {
  return /A line name cannot (be empty|contain control)|A line named ".*" already exists/.test(
    error ?? ''
  );
}
