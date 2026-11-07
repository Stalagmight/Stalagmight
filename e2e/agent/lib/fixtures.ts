import * as fs from 'fs';
import * as path from 'path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'url';
import type { DatabaseSync as DatabaseSyncType } from 'node:sqlite';

import { foldLocalPaths } from './shareableFailure';

/**
 * `node:sqlite` is loaded through `createRequire` rather than a static import
 * because vite CANNOT resolve it. Vite's builtin check strips the `node:`
 * prefix and looks the bare name up in `module.builtinModules` - and
 * `node:sqlite`, being prefix-only, appears there under its prefixed name only
 * (`builtinModules.includes('sqlite') === false` on Node 24.13). So vite decides
 * it is a package called `sqlite` and vitest dies with "Failed to load url
 * sqlite". No vite/vitest option fixes that; it is below the externalization
 * layer. A runtime `require` is invisible to the transform, and the type-only
 * import above keeps this fully typed. Harness-only - the app bundle never
 * touches SQLite from JS.
 */
const nodeRequire = createRequire(import.meta.url);
const { DatabaseSync } = nodeRequire('node:sqlite') as { DatabaseSync: typeof DatabaseSyncType };

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

export const DEFAULT_FIXTURE = 'fractal_plant_iter3.stalx';

export function fixtureSourcePath(name: string): string {
  return path.join(repoRoot, 'e2e', 'fixtures', path.basename(name));
}

/** Copy a tracked fixture before the app can migrate or otherwise mutate it. */
export function materializeFixture(name: string, destinationDir: string): string {
  const source = fixtureSourcePath(name);
  // The name is the caller's - a scenario's constant, but also a knob, an MCP
  // argument or a trace's `reload` step - so it goes on a later line, which the
  // runners withhold (#576 W51).
  if (!fs.existsSync(source)) throw new Error(`fixture missing from e2e/fixtures/\n  name: ${path.basename(name)}`);

  fs.mkdirSync(destinationDir, { recursive: true });
  const destination = path.join(destinationDir, path.basename(name));
  fs.copyFileSync(source, destination);
  return destination;
}

/**
 * The tables `Database::clear_undo_history_at` empties, in its order.
 * Kept literally in step with `src-tauri/src/persistence/sqlite/undo.rs`.
 */
export const UNDO_HISTORY_TABLES: readonly string[] = [
  'undo_stack',
  'redo_stack',
  'speleo_undo_chain',
  'speleo_undo_watermark',
];

export interface StripUndoHistoryResult {
  /** Rows deleted, per table that was present. */
  cleared: Record<string, number>;
  /** Tables that did not exist (an older snapshot shape simply has none). */
  missing: string[];
}

/**
 * Empty the undo/redo tables in a `.stalx`, so a fixture opens with NO inherited
 * history.
 *
 * A `.stalx` is a plain SQLite database, and tracked fixtures ship with a
 * persisted undo stack (`undo_persist_limit`, 20 by default): the fractal
 * fixture carries 20 rows, naharon 2. That history is real state a scenario may
 * want, but a scenario that models undo depth does not: it would be fuzzing on
 * top of somebody else's entries and burning the headroom below the 100-entry
 * history limit.
 *
 * This is the harness-side replica of `Database::clear_undo_history_at`
 * (`src-tauri/src/persistence/sqlite/undo.rs`, the download-replace
 * chokepoint's inherited-history drop): a bare connection, one transaction,
 * `DELETE FROM` each table that exists, a missing table is not an error. Same
 * table list, same order, same tolerance - the app's own recipe rather than a
 * parallel invention.
 *
 * MUST be pointed at a materialized COPY. `materializeFixture` is what produces
 * one; running this on `e2e/fixtures/` itself would mutate a tracked file, so
 * the path is refused if it resolves anywhere inside the tracked fixture
 * directory - at any depth, and through a symlink. The earlier form compared
 * against `join(trackedDir, basename(resolved))`, which is only the DIRECT
 * children: `e2e/fixtures/sub/x.stalx` walked straight past it into the
 * `DELETE FROM`, and so did a symlink pointing at a tracked file.
 *
 * (`node:sqlite` is still flagged experimental in Node 24, so importing this
 * module emits one `ExperimentalWarning` on stderr.)
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

export function stripUndoHistory(databasePath: string): StripUndoHistoryResult {
  const resolved = path.resolve(databasePath);
  if (isInsideTrackedFixtures(resolved)) {
    throw new Error(`refusing to strip undo history from the tracked fixture ${foldLocalPaths(resolved)}; copy it first`);
  }
  if (!fs.existsSync(resolved)) throw new Error(`no such .stalx: ${foldLocalPaths(resolved)}`);

  const cleared: Record<string, number> = {};
  const missing: string[] = [];
  const db = new DatabaseSync(resolved);
  try {
    db.exec('BEGIN');
    try {
      const tableExists = db.prepare(
        "SELECT COUNT(*) AS count FROM sqlite_master WHERE type='table' AND name=?",
      );
      for (const table of UNDO_HISTORY_TABLES) {
        const row = tableExists.get(table) as { count: number } | undefined;
        if (!row || Number(row.count) === 0) {
          missing.push(table);
          continue;
        }
        // Table names come from the frozen list above, never from a caller.
        const before = db.prepare(`SELECT COUNT(*) AS count FROM ${table}`).get() as { count: number };
        db.exec(`DELETE FROM ${table}`);
        cleared[table] = Number(before.count);
      }
      db.exec('COMMIT');
    } catch (err) {
      db.exec('ROLLBACK');
      throw err;
    }
  } finally {
    db.close();
  }
  return { cleared, missing };
}

/** Materialize a fixture into `destinationDir` and drop its inherited undo history. */
export function materializeFixtureWithoutHistory(
  name: string,
  destinationDir: string,
): { path: string; stripped: StripUndoHistoryResult } {
  const materialized = materializeFixture(name, destinationDir);
  return { path: materialized, stripped: stripUndoHistory(materialized) };
}
