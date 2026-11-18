/**
 * Client-side checks for the names the backend requires (SPEC 4.1/4.2): a line
 * name is required and project-unique, a station name is required and unique
 * within its line. The backend refuses the same cases (`validate_line_name`,
 * `validate_station_name`: trimmed, exact match); these let a dialog say so
 * inline and disable its submit before the round trip. A backend refusal is
 * still surfaced, because another window or an undo can change the names.
 */
import i18n from '../i18n';

/** Why a name cannot be used: none given, or another line / station has it. */
export type NameProblem = 'empty' | 'duplicate';

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
  const taken = (lines ?? []).some((l) => l.id !== exceptLineId && (l.name ?? '').trim() === trimmed);
  return taken ? 'duplicate' : null;
}

/**
 * Whether `name` can name station `stationIndex` of a line whose stations are
 * named `lineStationNames` (by index). Trimmed, exact.
 */
export function stationNameProblem(
  name: string,
  lineStationNames: ReadonlyArray<string>,
  stationIndex: number,
): NameProblem | null {
  const trimmed = name.trim();
  if (trimmed === '') return 'empty';
  return lineStationNames.some((n, i) => i !== stationIndex && n === trimmed) ? 'duplicate' : null;
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
    case 'duplicate':
      return i18n.t('identity.lineNameTaken', { name: name.trim() });
    case 'same':
      return i18n.t('identity.lineNamesSame');
  }
}

/**
 * Whether a backend error is its refusal of a line name (`validate_line_name`:
 * "A line name cannot be empty", `A line named "X" already exists`), so the
 * dialog shows it at the name field rather than as a general failure.
 */
export function isLineNameRefusal(error: string | null | undefined): boolean {
  return /A line name cannot be empty|A line named ".*" already exists/.test(error ?? '');
}
