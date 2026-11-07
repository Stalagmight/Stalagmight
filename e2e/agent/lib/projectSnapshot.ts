/**
 * NODE-SIDE half of the undo/redo fuzzer's oracle: the diff reporter and the
 * attribution classifier.
 *
 * The canonical snapshot itself is taken IN THE WEBVIEW - see
 * `fuzzOracleCore.ts`, which owns the serializer and documents what it
 * normalizes. Only the rare, slow paths live here: when two canonical strings
 * disagree, Node fetches both and works out whether the difference is real,
 * floating-point dust, or one of the two DOCUMENTED, INTENDED families a save
 * or a reload is allowed to produce (`SAVE_HEALING_MUTATIONS`,
 * `RELOAD_RESOLVED_GEOMETRY`). Both of those are narrowed by SHAPE rather than
 * by magnitude - `isNumericMove` rejects anything that is not a number moving
 * to another number, and how FAR it moved is reported (`maxNumericDelta`)
 * instead of bounded.
 *
 * ## No known-bug classifiers live here any more
 *
 * This file used to carry `KNOWN_UNDO_ASYMMETRIES` (issue #413 findings 1 and
 * 2), `LOOP_CLOSURE_REORDER` (finding 4), `PRE_SAVE_SNAPSHOT_HEAL_REVERT`
 * (finding 11) and `isShotIdentitySwap` (finding 8's tolerance gate). Every one
 * of those findings is fixed in the Rust tier, so each classifier was DELETED
 * per the delete-when-fixed contract the fix waves follow: a classifier that
 * outlives its bug is a tolerance nobody re-examines. What remains is
 * attribution for behaviour the app is designed to have, and it is narrow by
 * path AND by direction.
 *
 * ## Why there is a tolerance at all
 *
 * Station coordinates and section offsets are RE-DERIVED after an undo/redo
 * (`reconcile_geometry()` -> `compute_station_coordinates`), not copied, so a
 * value whose true result is 0 can land on different floating-point dust
 * depending on the order the survey chain was summed in. Observed in practice:
 * `stations[].coordinates.x` coming back as -3.55e-15 where the recorded
 * snapshot had -7.11e-15 (femtometres, either side of a true zero) after `undo`
 * of a `moveStation`.
 *
 * The canonical string is rounded to 12 significant digits, which usually makes
 * the strings match outright; when they do not, `diffJson` compares numbers
 * against `DEFAULT_TOLERANCE` - 1e-9 ABSOLUTE (a nanometre; this is what catches
 * the near-zero case, which no relative bound can) plus 1e-12 relative. A real
 * geometry regression is millimetres at the very least, so nothing a bug could
 * produce is inside that envelope; a caller that wants bit-exactness passes
 * `tolerance: null`.
 */

/**
 * A named, path-and-direction-narrow rule for attributing a snapshot
 * difference to something the app is DOCUMENTED to do without an undo entry -
 * today only the save-time heals and the reload's offset re-solve.
 *
 * The name is historical: these used to double as known-open-bug classifiers,
 * and none of those are left (see the module header). A rule here never makes a
 * comparison green on its own - the scenarios apply them only to comparisons
 * that CROSS a save/reload epoch, and re-baseline rather than ignore.
 */
export interface KnownAsymmetry {
  id: string;
  /** Matched against a diff path such as `project.sections[3].shots[1].uuid`. */
  pathPattern: RegExp;
  /**
   * Optional extra narrowing on the VALUES, and the reason a rule here can be
   * trusted to subtract only what it names. A path pattern alone cannot tell
   * "the save filled in an anchor that was null" from "an anchor the app had
   * was LOST", nor "a reload re-solved an offset" from "an offset entry
   * appeared or vanished" - and in each pair only the first is the documented,
   * intended behaviour. `isNumericMove` and the per-rule `accepts` below are
   * what draw that line; a difference on an attributable path that fails them
   * is not attributed, and still fails the run.
   */
  accepts?(diff: SnapshotDiff): boolean;
  description: string;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Deterministic JSON for an already-canonical value (keys are pre-sorted). */
export function canonicalJson(value: unknown): string {
  return JSON.stringify(value ?? null);
}

export interface SnapshotDiff {
  path: string;
  expected: string;
  actual: string;
}

/** Numeric closeness used by the diff: `|a-b| <= absolute + relative*max(|a|,|b|)`. */
export interface NumericTolerance {
  absolute: number;
  relative: number;
}

/** See the module header: a nanometre absolute, 1e-12 relative. */
export const DEFAULT_TOLERANCE: NumericTolerance = { absolute: 1e-9, relative: 1e-12 };

function numbersEqual(a: number, b: number, tolerance: NumericTolerance | null): boolean {
  if (a === b) return true;
  if (!tolerance) return false;
  if (!Number.isFinite(a) || !Number.isFinite(b)) return false;
  return Math.abs(a - b) <= tolerance.absolute + tolerance.relative * Math.max(Math.abs(a), Math.abs(b));
}

const ABSENT = '<absent>';

function show(value: unknown, maxChars = 160): string {
  if (value === undefined) return ABSENT;
  let text: string;
  try {
    text = JSON.stringify(value) ?? String(value);
  } catch {
    text = String(value);
  }
  return text.length > maxChars ? `${text.slice(0, maxChars)}...[${text.length} chars]` : text;
}

function walkDiff(
  expected: unknown,
  actual: unknown,
  path: string,
  out: SnapshotDiff[],
  limit: number,
  tolerance: NumericTolerance | null,
): void {
  if (out.length >= limit) return;
  const label = path || '<root>';

  if (Array.isArray(expected) && Array.isArray(actual)) {
    if (expected.length !== actual.length) {
      out.push({ path: `${label}.length`, expected: String(expected.length), actual: String(actual.length) });
      if (out.length >= limit) return;
    }
    const shared = Math.min(expected.length, actual.length);
    for (let i = 0; i < shared; i++) {
      walkDiff(expected[i], actual[i], `${path}[${i}]`, out, limit, tolerance);
      if (out.length >= limit) return;
    }
    return;
  }

  if (isPlainObject(expected) && isPlainObject(actual)) {
    const keys = [...new Set([...Object.keys(expected), ...Object.keys(actual)])].sort();
    for (const key of keys) {
      const childPath = path ? `${path}.${key}` : key;
      const inExpected = key in expected;
      const inActual = key in actual;
      if (!inExpected || !inActual) {
        out.push({
          path: childPath,
          expected: inExpected ? show(expected[key]) : ABSENT,
          actual: inActual ? show(actual[key]) : ABSENT,
        });
      } else {
        walkDiff(expected[key], actual[key], childPath, out, limit, tolerance);
      }
      if (out.length >= limit) return;
    }
    return;
  }

  if (typeof expected === 'number' && typeof actual === 'number') {
    if (!numbersEqual(expected, actual, tolerance)) {
      out.push({ path: label, expected: show(expected), actual: show(actual) });
    }
    return;
  }

  if (expected !== actual) {
    out.push({ path: label, expected: show(expected), actual: show(actual) });
  }
}

/**
 * First `limit` differences between two normalized snapshots. Numbers within
 * `tolerance` are treated as equal; pass `null` for bit-exact comparison.
 */
export function diffJson(
  expected: unknown,
  actual: unknown,
  limit = 6,
  tolerance: NumericTolerance | null = DEFAULT_TOLERANCE,
): SnapshotDiff[] {
  const out: SnapshotDiff[] = [];
  walkDiff(expected, actual, '', out, limit, tolerance);
  return out;
}

/** Diff two canonical snapshot strings (parses them back first). */
export function diffSnapshots(
  expectedJson: string,
  actualJson: string,
  limit = 6,
  tolerance: NumericTolerance | null = DEFAULT_TOLERANCE,
): SnapshotDiff[] {
  const parse = (text: string): unknown => {
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  };
  return diffJson(parse(expectedJson), parse(actualJson), limit, tolerance);
}

export interface DiffClassification {
  /** Diffs explained by one of the rules passed in. */
  known: { diff: SnapshotDiff; asymmetry: KnownAsymmetry }[];
  /** Diffs nothing in that list accounts for. */
  unknown: SnapshotDiff[];
  /** Distinct rules hit. */
  asymmetries: KnownAsymmetry[];
}

/**
 * `known` is REQUIRED. It used to default to `KNOWN_UNDO_ASYMMETRIES`, which is
 * deleted (every finding it named is fixed); making the list explicit is what
 * stops a future caller silently re-acquiring a default tolerance.
 */
export function classifyDiffs(
  diffs: readonly SnapshotDiff[],
  known: readonly KnownAsymmetry[],
): DiffClassification {
  const matched: DiffClassification['known'] = [];
  const unknown: SnapshotDiff[] = [];
  const seen = new Map<string, KnownAsymmetry>();
  for (const diff of diffs) {
    const hit = known.find(
      (candidate) => candidate.pathPattern.test(diff.path) && (candidate.accepts?.(diff) ?? true),
    );
    if (hit) {
      matched.push({ diff, asymmetry: hit });
      seen.set(hit.id, hit);
    } else {
      unknown.push(diff);
    }
  }
  return { known: matched, unknown, asymmetries: [...seen.values()] };
}

/** Path segments to a list value inside a snapshot. */
export type ListPath = readonly string[];

/** Where the closure list lives in a `get_project_state` payload. */
export const LOOP_CLOSURES_PATH: ListPath = ['project', 'loop_closures'];

function readPath(root: unknown, segments: ListPath): unknown {
  let cursor = root;
  for (const segment of segments) {
    if (!isPlainObject(cursor)) return undefined;
    cursor = cursor[segment];
  }
  return cursor;
}

function writePath(root: unknown, segments: ListPath, value: unknown): void {
  let cursor = root;
  for (const segment of segments.slice(0, -1)) {
    if (!isPlainObject(cursor)) return;
    cursor = cursor[segment];
  }
  const last = segments[segments.length - 1];
  if (isPlainObject(cursor) && last !== undefined) cursor[last] = value;
}

/** Total, content-derived order - no assumption about which field is the key. */
function sortedByCanonical(items: readonly unknown[]): unknown[] {
  return items
    .map((item) => [canonicalJson(item), item] as const)
    .sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))
    .map(([, item]) => item);
}

/**
 * True when two snapshots hold the SAME list members at `listPath` in a
 * DIFFERENT order and are otherwise identical.
 *
 * DIAGNOSTIC ONLY, and now purely so. It never made a comparison pass - the
 * snapshots are still unequal and the fuzzer still fails - but it used to be the
 * recognizer behind the `undo-remove-loop-closure-appends-instead-of-reinserting`
 * classifier (issue #413 finding 4), which is deleted along with the bug. What
 * survives is the READING of the failure: "undo reordered the closure list"
 * reported as one sentence instead of 30 unrelated-looking value diffs at
 * shifted indices, should anything ever reintroduce that shape.
 */
export function isListPermutationOnly(
  expectedJson: string,
  actualJson: string,
  listPath: ListPath,
): boolean {
  let expected: unknown;
  let actual: unknown;
  try {
    expected = JSON.parse(expectedJson);
    actual = JSON.parse(actualJson);
  } catch {
    return false;
  }
  const expectedList = readPath(expected, listPath);
  const actualList = readPath(actual, listPath);
  if (!Array.isArray(expectedList) || !Array.isArray(actualList)) return false;
  if (expectedList.length !== actualList.length) return false;
  // Already in the same order: whatever differs, it is not a permutation.
  if (canonicalJson(expectedList) === canonicalJson(actualList)) return false;

  writePath(expected, listPath, sortedByCanonical(expectedList));
  writePath(actual, listPath, sortedByCanonical(actualList));
  return diffJson(expected, actual).length === 0;
}

/** Human-readable diff block for a failure message. */
export function describeDiffs(diffs: readonly SnapshotDiff[]): string {
  if (diffs.length === 0) return '  (no structural difference found - the canonical strings differ only in encoding)';
  return diffs
    .map((diff) => `  at ${diff.path}\n    expected: ${diff.expected}\n    actual:   ${diff.actual}`)
    .join('\n');
}

/**
 * Changes a SAVE is documented to make to live project state, with no undo entry
 * of its own (`AppState::save_project`, src-tauri/src/core/state/project_io.rs).
 *
 * These are healing mutations: the save settles identity and drops dead data
 * before writing. They are real state changes the roundtrip model has to account
 * for, but they are ATTRIBUTABLE - so a save whose diff is entirely explained by
 * this list is absorbed (and recorded), while a save that changes ANYTHING else
 * is a hard failure. That is the oracle: "save mutated the project in an
 * undocumented way".
 *
 * Deliberately narrow, by value as well as by path: a gauge anchor may only go
 * from null to a value (capture_missing_*), a uuid may only be filled in, an
 * offset entry may only disappear (the orphan retain), and a section's creation
 * time may only appear, as an RFC 3339 stamp. A gauge anchor CHANGING from one
 * value to another, an offset entry appearing, or a creation time changing, is
 * not on this list.
 */
export const SAVE_HEALING_MUTATIONS: readonly KnownAsymmetry[] = [
  {
    id: 'save-captures-missing-gauge-anchors',
    pathPattern: /^project\.sections\[\d+\]\.gauge_(shot|neighbour)_uuid$/,
    accepts: (diff) => diff.expected === 'null' && diff.actual !== 'null',
    description:
      'save_project calls capture_missing_gauge_anchors(), which records the least-squares ' +
      'gauge pair for any section that lacks one (project_io.rs). Null -> value only; it never ' +
      'moves coordinates.',
  },
  {
    id: 'save-backfills-shot-uuids',
    pathPattern: /^project\.sections\[\d+\]\.shots\[\d+\]\.uuid$/,
    accepts: (diff) => (diff.expected === ABSENT || diff.expected === 'null') && diff.actual !== ABSENT,
    description:
      'save_project calls ensure_shot_uuids(), minting a uuid for any shot still missing one ' +
      '(pre-v6 files). Absent -> value only.',
  },
  {
    id: 'save-drops-orphan-section-offsets',
    pathPattern: /^project\.section_offsets\.\d+$/,
    accepts: (diff) => diff.actual === ABSENT,
    description:
      'save_project retains only section_offsets whose section still exists - the SQLite row ' +
      'carries a FOREIGN KEY to sections(id), so an orphan would abort the whole save. ' +
      'Disappearance only.',
  },
  {
    id: 'save-stamps-section-creation-times',
    pathPattern: /^project\.sections\[\d+\]\.created_at$/,
    accepts: (diff) => diff.expected === ABSENT && isRfc3339Text(diff.actual),
    description:
      'save_project calls stamp_section_creation_times() (project_io.rs, #576 NF-W14BE-1), giving ' +
      'a section with no creation time (new, or its stored one unreadable) the save time, on the ' +
      'model. Absent -> an RFC 3339 stamp only: a stamp the section already has is written back ' +
      'byte for byte (#576 RV14-2), so one changing or going is not this.',
  },
];

/**
 * True when `rendered` (a diff value as `show` renders it: JSON text) is a JSON
 * string holding an RFC 3339 date-time that names a real moment.
 */
function isRfc3339Text(rendered: string): boolean {
  let value: unknown;
  try {
    value = JSON.parse(rendered);
  } catch {
    return false;
  }
  if (typeof value !== 'string') return false;
  const match = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/.exec(value);
  if (!match) return false;
  const [, , month, day, hour, minute, second] = match.map(Number);
  const inRange =
    month !== undefined && month >= 1 && month <= 12 &&
    day !== undefined && day >= 1 && day <= 31 &&
    hour !== undefined && hour <= 23 &&
    minute !== undefined && minute <= 59 &&
    second !== undefined && second <= 60;
  return inRange && !Number.isNaN(Date.parse(value));
}

/**
 * Derived geometry a RELOAD re-solves from scratch.
 *
 * `Project::redrive_offsets_after_load` (src-tauri/src/models/project/offsets.rs:186-188)
 * ends with an unconditional `resolve_offsets()` under the comment "Zero-trust:
 * never honor a persisted derivable offset - re-solve unconditionally so a
 * sub-tolerance stale offset can't survive the load." So `section_offsets` and
 * the station coordinates derived from them are NOT restored from the file -
 * they are recomputed - and a snapshot recorded in-session before the save can
 * legitimately hold a drifted value the reload then corrects. (In this fuzzer's
 * own runs the drift is usually bug 3's or bug 7's, which is exactly the kind of
 * corruption a reload is designed to heal.)
 *
 * Deliberately scoped to those two derived families and applied ONLY to
 * comparisons that cross a reload. Everything else - section and shot fields,
 * uuids, merges, closures, datums, comments, stats - must still match exactly,
 * so this cannot hide a structural divergence.
 *
 * Narrowed by SHAPE, not by magnitude, and the distinction is deliberate. A
 * re-solve MOVES a number: an offset entry appearing or disappearing, or a
 * coordinate turning into null, is structural and is not this - `isNumericMove`
 * rejects those. What is NOT bounded is how far the number moved, because the
 * heal this family exists to absorb is measured in metres: bug 3 displaced a
 * whole section by a shot's horizontal vector (1.13 m in its repro) and
 * RATCHETED on every cycle, and the reload correcting that was the documented
 * case above. Any sub-metre threshold would fail on the heal; any threshold
 * loose enough to pass it would not catch what a threshold is for. So the
 * magnitude is REPORTED instead - `maxNumericDelta` puts it in the attributed
 * line and in a metric - and a number that starts growing is visible rather
 * than silent.
 */
export const RELOAD_RESOLVED_GEOMETRY: readonly KnownAsymmetry[] = [
  {
    id: 'reload-resolves-section-offsets',
    pathPattern: /^project\.section_offsets\.\d+(\[\d+\])?$/,
    accepts: isNumericMove,
    description:
      'A reload re-solves section_offsets unconditionally rather than trusting the persisted ' +
      'value (offsets.rs redrive_offsets_after_load). A numeric MOVE only: an entry appearing ' +
      'or disappearing is structural and is not this.',
  },
  {
    id: 'reload-resolves-station-coordinates',
    pathPattern: /^stations\[\d+\]\.coordinates\.[xyz]$/,
    accepts: isNumericMove,
    description:
      'Station coordinates are derived from section_offsets, so they move with the re-solve a ' +
      'reload performs. A numeric MOVE only.',
  },
];

/** Every finite number in a rendered diff value (`3.5`, `[1,2]`, `<absent>` -> none). */
function numbersIn(text: string): number[] {
  if (text === ABSENT) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return [];
  }
  if (typeof parsed === 'number') return Number.isFinite(parsed) ? [parsed] : [];
  if (Array.isArray(parsed)) {
    const out: number[] = [];
    for (const entry of parsed) {
      if (typeof entry !== 'number' || !Number.isFinite(entry)) return [];
      out.push(entry);
    }
    return out;
  }
  return [];
}

/**
 * Is this diff a number (or a tuple of numbers) MOVING to another one?
 *
 * The re-derived-geometry families are about a value being recomputed. A path
 * in one of them that appears, disappears, or changes type is a structural
 * change wearing a geometry path, and must not be absorbed as a re-solve.
 */
export function isNumericMove(diff: SnapshotDiff): boolean {
  const before = numbersIn(diff.expected);
  const after = numbersIn(diff.actual);
  return before.length > 0 && before.length === after.length;
}

/**
 * The largest absolute change across `diffs`, or null when none is numeric.
 *
 * Reported alongside an absorbed geometry difference so its SIZE is on the
 * record: this family has no magnitude bound (see `RELOAD_RESOLVED_GEOMETRY`),
 * which is only defensible while the number is visible.
 */
export function maxNumericDelta(diffs: readonly SnapshotDiff[]): number | null {
  let worst: number | null = null;
  for (const diff of diffs) {
    const before = numbersIn(diff.expected);
    const after = numbersIn(diff.actual);
    if (before.length === 0 || before.length !== after.length) continue;
    for (const [index, value] of before.entries()) {
      const delta = Math.abs(value - (after[index] ?? 0));
      if (worst === null || delta > worst) worst = delta;
    }
  }
  return worst;
}

/** Per-section shot-identity delta between two canonical snapshots. */
export interface ShotSetDelta {
  sectionId: number;
  /** Shot uuids present in `expected` but missing from `actual`. */
  missing: string[];
  /** Shot uuids present in `actual` but missing from `expected`. */
  extra: string[];
  expectedCount: number;
  actualCount: number;
}

/**
 * Which SHOTS (by uuid, the identity that survives a save/reload) a section
 * gained or lost between two snapshots.
 *
 * A shifted shot list produces dozens of field-level diffs that all look like
 * value changes, and those cannot distinguish "the same shots moved" from "a
 * different shot was removed". This answers that directly, which is the
 * difference between a reordering and data loss.
 *
 * Sections present on only ONE side count as the whole section's shots gained
 * or lost. They are the LARGEST identity change there is, and skipping them -
 * which an earlier version did, by iterating `expected` and `continue`ing when
 * the id was missing from `actual` - reported an EMPTY delta for a run that
 * dropped an entire section, whereupon the caller printed "shot identity by
 * uuid is IDENTICAL on both sides - the shots moved, none was lost".
 */
export function shotSetDelta(expectedJson: string, actualJson: string): ShotSetDelta[] {
  const read = (text: string): Map<number, string[]> => {
    const out = new Map<number, string[]>();
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch {
      return out;
    }
    const project = isPlainObject(parsed) ? parsed.project : null;
    if (!isPlainObject(project) || !Array.isArray(project.sections)) return out;
    for (const section of project.sections) {
      if (!isPlainObject(section) || !Array.isArray(section.shots)) continue;
      const uuids: string[] = [];
      for (const shot of section.shots) {
        if (isPlainObject(shot)) uuids.push(typeof shot.uuid === 'string' ? shot.uuid : '<no-uuid>');
      }
      out.set(Number(section.id), uuids);
    }
    return out;
  };

  const expected = read(expectedJson);
  const actual = read(actualJson);
  const deltas: ShotSetDelta[] = [];
  const sectionIds = [...new Set([...expected.keys(), ...actual.keys()])].sort((a, b) => a - b);
  for (const sectionId of sectionIds) {
    const expectedUuids = expected.get(sectionId) ?? [];
    const actualUuids = actual.get(sectionId) ?? [];
    const actualSet = new Set(actualUuids);
    const expectedSet = new Set(expectedUuids);
    const missing = expectedUuids.filter((uuid) => !actualSet.has(uuid));
    const extra = actualUuids.filter((uuid) => !expectedSet.has(uuid));
    if (missing.length > 0 || extra.length > 0) {
      deltas.push({
        sectionId,
        missing,
        extra,
        expectedCount: expectedUuids.length,
        actualCount: actualUuids.length,
      });
    }
  }
  return deltas;
}

/** Render a shot-set delta for a failure message. */
export function describeShotSetDelta(deltas: readonly ShotSetDelta[]): string {
  return deltas
    .map(
      (delta) =>
        `  section ${delta.sectionId}: ${delta.expectedCount} shot(s) expected, ${delta.actualCount} present\n` +
        `    shots MISSING (present before, gone now): ${JSON.stringify(delta.missing)}\n` +
        `    shots UNEXPECTED (absent before, present now): ${JSON.stringify(delta.extra)}`,
    )
    .join('\n');
}
