import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { createRequire } from 'node:module';
import type { DatabaseSync as DatabaseSyncType } from 'node:sqlite';
import { describe, expect, it } from 'vitest';

import {
  UNDO_HISTORY_TABLES,
  fixtureSourcePath,
  isInsideTrackedFixtures,
  stripUndoHistory,
} from './fixtures';

// Loaded at runtime, not imported: see the note in fixtures.ts (vite cannot
// resolve the prefix-only `node:sqlite`).
const { DatabaseSync } = createRequire(import.meta.url)('node:sqlite') as {
  DatabaseSync: typeof DatabaseSyncType;
};

/**
 * Build a throwaway SQLite file shaped like the undo/redo half of a `.stalx`.
 * Deliberately synthetic: no tracked fixture is read, copied or mutated here.
 */
function makeStalxLike(
  dir: string,
  options: { tables?: readonly string[]; rowsPerTable?: number; extraTable?: boolean } = {},
): string {
  const tables = options.tables ?? UNDO_HISTORY_TABLES;
  const rows = options.rowsPerTable ?? 3;
  const file = path.join(dir, 'synthetic.stalx');
  const db = new DatabaseSync(file);
  for (const table of tables) {
    db.exec(`CREATE TABLE ${table} (sequence INTEGER PRIMARY KEY, payload TEXT)`);
    for (let i = 0; i < rows; i++) {
      db.prepare(`INSERT INTO ${table} (sequence, payload) VALUES (?, ?)`).run(i, `entry-${i}`);
    }
  }
  if (options.extraTable !== false) {
    // Project data must survive untouched - only the history tables are cleared.
    db.exec('CREATE TABLE sections (id INTEGER PRIMARY KEY, name TEXT)');
    db.prepare('INSERT INTO sections (id, name) VALUES (?, ?)').run(1, 'keep-me');
  }
  db.close();
  return file;
}

function countRows(file: string, table: string): number {
  const db = new DatabaseSync(file, { readOnly: true });
  try {
    const row = db.prepare(`SELECT COUNT(*) AS count FROM ${table}`).get() as { count: number };
    return Number(row.count);
  } finally {
    db.close();
  }
}

function tableNames(file: string): string[] {
  const db = new DatabaseSync(file, { readOnly: true });
  try {
    return (db.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").all() as {
      name: string;
    }[]).map((row) => row.name);
  } finally {
    db.close();
  }
}

function tempDir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-undo-strip-'));
}

describe('stripUndoHistory', () => {
  it('clears every history table and reports the row counts it removed', () => {
    const file = makeStalxLike(tempDir());

    const result = stripUndoHistory(file);

    expect(result.missing).toEqual([]);
    expect(result.cleared).toEqual({
      undo_stack: 3,
      redo_stack: 3,
      speleo_undo_chain: 3,
      speleo_undo_watermark: 3,
    });
    for (const table of UNDO_HISTORY_TABLES) expect(countRows(file, table)).toBe(0);
  });

  it('leaves the project data alone', () => {
    const file = makeStalxLike(tempDir());

    stripUndoHistory(file);

    // Tables are emptied, never dropped (the app reopens and writes into them).
    expect(tableNames(file)).toEqual([...UNDO_HISTORY_TABLES, 'sections'].sort());
    expect(countRows(file, 'sections')).toBe(1);
  });

  it('treats a missing table as fine, matching clear_undo_history_at', () => {
    // The shape both tracked fixtures actually have: no speleo_* tables.
    const file = makeStalxLike(tempDir(), { tables: ['undo_stack', 'redo_stack'] });

    const result = stripUndoHistory(file);

    expect(result.cleared).toEqual({ undo_stack: 3, redo_stack: 3 });
    expect(result.missing).toEqual(['speleo_undo_chain', 'speleo_undo_watermark']);
  });

  it('is idempotent', () => {
    const file = makeStalxLike(tempDir());

    stripUndoHistory(file);
    const second = stripUndoHistory(file);

    expect(second.cleared).toEqual({
      undo_stack: 0,
      redo_stack: 0,
      speleo_undo_chain: 0,
      speleo_undo_watermark: 0,
    });
  });

  it('handles a database with no history tables at all', () => {
    const file = makeStalxLike(tempDir(), { tables: [] });

    const result = stripUndoHistory(file);

    expect(result.cleared).toEqual({});
    expect(result.missing).toEqual([...UNDO_HISTORY_TABLES]);
  });

  it('refuses to touch a tracked fixture, only a materialized copy', () => {
    expect(() => stripUndoHistory(fixtureSourcePath('fractal_plant_iter3.stalx'))).toThrow(
      /refusing to strip undo history from the tracked fixture/,
    );
  });

  it('refuses a tracked fixture reached through a SYMLINK', () => {
    // The guard used to compare against `join(trackedDir, basename(resolved))`,
    // so any path whose basename matched but whose directory did not walked
    // straight into the DELETE FROM - including a link to the real file.
    const tracked = fixtureSourcePath('fractal_plant_iter3.stalx');
    const link = path.join(tempDir(), 'looks-like-a-copy.stalx');
    fs.symlinkSync(tracked, link);

    expect(() => stripUndoHistory(link)).toThrow(/refusing to strip undo history/);
  });

  it('refuses a NESTED path under the tracked fixture directory', () => {
    // e2e/fixtures/ is flat today, so this is latent rather than live - which is
    // exactly when a containment check is cheap to get right.
    expect(isInsideTrackedFixtures(path.join(fixtureSourcePath('x.stalx'), '..', 'sub', 'y.stalx'))).toBe(
      true,
    );
    expect(isInsideTrackedFixtures(fixtureSourcePath('naharon.stalx'))).toBe(true);
  });

  it('allows a materialized copy outside that directory', () => {
    // The negative half: a containment check that refuses everything would
    // pass every test above and break every scenario.
    expect(isInsideTrackedFixtures(path.join(tempDir(), 'naharon.stalx'))).toBe(false);
  });

  it('reports a missing file rather than creating an empty database', () => {
    const missing = path.join(tempDir(), 'nope.stalx');

    expect(() => stripUndoHistory(missing)).toThrow(/no such \.stalx/);
    expect(fs.existsSync(missing)).toBe(false);
  });
});
