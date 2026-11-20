import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

export const DEFAULT_FIXTURE = 'fractal_plant_iter3.stalx';

export function fixtureSourcePath(name: string): string {
  return path.join(repoRoot, 'e2e', 'fixtures', path.basename(name));
}

/** Copy a tracked fixture before the app can migrate or otherwise mutate it. */
export function materializeFixture(name: string, destinationDir: string): string {
  const source = fixtureSourcePath(name);
  if (!fs.existsSync(source)) throw new Error(`fixture missing: ${source}`);

  fs.mkdirSync(destinationDir, { recursive: true });
  const destination = path.join(destinationDir, path.basename(name));
  fs.copyFileSync(source, destination);
  return destination;
}

/** What a `.stalx` holds, by its first bytes (plan 6.1's sniff). */
export type StalxFormat = 'legacy' | 'v2';

const SQLITE_MAGIC = 'SQLite format 3\u0000';

/**
 * The format of the `.stalx` at `file`: a legacy SQLite database (what every
 * build before the station-graph switch wrote, and what the tracked fixtures
 * still are) or version 2 JSON. Anything else throws.
 */
export function sniffStalx(file: string): StalxFormat {
  const fd = fs.openSync(file, 'r');
  try {
    const head = Buffer.alloc(64);
    const read = fs.readSync(fd, head, 0, head.length, 0);
    const text = head.subarray(0, read).toString('latin1');
    if (text.startsWith(SQLITE_MAGIC)) return 'legacy';
    if (/^\{\s*"format"\s*:\s*"stalx"\s*,\s*"version"\s*:/.test(text)) return 'v2';
  } finally {
    fs.closeSync(fd);
  }
  throw new Error(`not a Stalagmight project file: ${file}`);
}

export interface StripUndoHistoryResult {
  /** The format the file was in. */
  format: StalxFormat;
  /** Entries removed, per stack (`undo`, `redo`) that held any. */
  cleared: Record<string, number>;
  /** Stacks that were already empty (a v2 file writes no empty stack). */
  missing: string[];
}

/**
 * Empty the undo/redo history of a `.stalx`, so a fixture opens with NO
 * inherited history.
 *
 * Tracked fixtures ship with a persisted undo stack. That history is real
 * state a scenario may want, but a scenario that models undo depth does not:
 * it would be fuzzing on top of somebody else's entries and burning the
 * headroom below the 100-entry history limit.
 *
 * - **Legacy (SQLite)**: nothing to do. The app migrates a legacy file in
 *   memory on open and DROPS its history (plan 7.2.8: the station-graph engine
 *   does not read the old entries), so a legacy fixture always opens with
 *   empty stacks. The file is left byte for byte as it was (the app's first
 *   save upgrades it), and the result says how it was found.
 * - **Version 2 (JSON)**: the `history` member is removed, which is exactly
 *   how the writer spells two empty stacks (`write_history` omits it), and
 *   every other byte is kept. The member is one top-level record that starts a
 *   line and ends right before `"end":true}` (plan 6.2's layout); a JSON
 *   string cannot hold a raw newline, so that line start is structural. The
 *   result is checked to parse with no history before it is written. The
 *   SpeleoDB unwind chain in `local` is not touched (a tracked fixture is not
 *   linked).
 *
 * MUST be pointed at a materialized COPY. `materializeFixture` is what produces
 * one; running this on `e2e/fixtures/` itself would mutate a tracked file, so
 * the path is refused if it resolves anywhere inside the tracked fixture
 * directory - at any depth, and through a symlink.
 */
/**
 * Does `resolved` live inside the TRACKED fixture directory?
 *
 * Containment, not equality with a re-joined basename: a nested path and a
 * symlink both pass the latter. `realpath` is applied to whatever exists (a
 * missing leaf is fine - it cannot be a tracked file), so a symlink under a
 * temp dir pointing at `e2e/fixtures/naharon.stalx` is caught by its target
 * rather than by its name. Compared case-insensitively off Linux, where
 * `.../NAHARON.stalx` opens the same file that `.../naharon.stalx` does.
 */
export function isInsideTrackedFixtures(resolved: string): boolean {
  const trackedDir = path.resolve(repoRoot, 'e2e', 'fixtures');
  const real = (target: string): string => {
    try {
      return fs.realpathSync(target);
    } catch {
      // Does not exist (yet): fall back to the lexical path, which is still
      // enough to catch `e2e/fixtures/whatever`.
      return target;
    }
  };
  const fold = (text: string): string => (process.platform === 'linux' ? text : text.toLowerCase());
  const candidate = fold(real(resolved));
  const root = fold(real(trackedDir));
  return candidate === root || candidate.startsWith(root + path.sep);
}

export function stripUndoHistory(stalxPath: string): StripUndoHistoryResult {
  const resolved = path.resolve(stalxPath);
  if (isInsideTrackedFixtures(resolved)) {
    throw new Error(`refusing to strip undo history from the tracked fixture ${resolved}; copy it first`);
  }
  if (!fs.existsSync(resolved)) throw new Error(`no such .stalx: ${resolved}`);

  const format = sniffStalx(resolved);
  if (format === 'legacy') {
    // The app drops it on open (plan 7.2.8); nothing on disk to change.
    return { format, cleared: {}, missing: ['undo', 'redo'] };
  }

  const text = fs.readFileSync(resolved, 'utf8');
  const parsed = JSON.parse(text) as { history?: { undo?: unknown[]; redo?: unknown[] } };
  const cleared: Record<string, number> = {};
  const missing: string[] = [];
  for (const stack of ['undo', 'redo'] as const) {
    const n = parsed.history?.[stack]?.length ?? 0;
    if (n > 0) cleared[stack] = n;
    else missing.push(stack);
  }
  if (parsed.history === undefined) return { format, cleared, missing };

  const end = text.lastIndexOf('\n"end":true}');
  const start = end < 0 ? -1 : text.lastIndexOf('\n"history":', end);
  if (start < 0) {
    throw new Error(`the history member of ${resolved} is not where the v2 layout puts it; refusing to rewrite`);
  }
  const stripped = text.slice(0, start) + text.slice(end);
  const check = JSON.parse(stripped) as { history?: unknown; end?: unknown };
  if (check.history !== undefined || check.end !== true) {
    throw new Error(`stripping the history of ${resolved} would not leave a valid project; refusing to rewrite`);
  }
  fs.writeFileSync(resolved, stripped);
  return { format, cleared, missing };
}

/** Materialize a fixture into `destinationDir` and drop its inherited undo history. */
export function materializeFixtureWithoutHistory(
  name: string,
  destinationDir: string,
): { path: string; stripped: StripUndoHistoryResult } {
  const materialized = materializeFixture(name, destinationDir);
  return { path: materialized, stripped: stripUndoHistory(materialized) };
}
