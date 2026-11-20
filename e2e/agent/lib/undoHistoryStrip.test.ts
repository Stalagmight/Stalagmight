import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

import { fixtureSourcePath, isInsideTrackedFixtures, sniffStalx, stripUndoHistory } from './fixtures';

/**
 * A throwaway `.stalx` version 2 file in the writer's layout (plan 6.2: one
 * record per line, `history` right before `"end":true}`). Deliberately
 * synthetic: no tracked fixture is read, copied or mutated here, and the
 * survey is three made-up stations.
 */
function makeV2(dir: string, options: { undo?: number; redo?: number } = {}): string {
  const entry = (k: number) => `{"v":1,"t":"2026-11-20T00:00:0${k}.000000000Z","k":"update_shot","label":"Edit shot ${k}","ops":{}}`;
  const stack = (n: number) => Array.from({ length: n }, (_, k) => entry(k)).join(',\n');
  const undo = options.undo ?? 3;
  const redo = options.redo ?? 2;
  const parts = [
    '{"format":"stalx","version":2,"solver":1,"requires":[],',
    '"project":{"name":"Synthetic, with a \\n\\"history\\": in its name"},',
    '"stations":[',
    '[1,"A"],',
    '[2,"B"]',
    '],',
  ];
  if (undo > 0 || redo > 0) {
    const members = [];
    if (undo > 0) members.push(`"undo":[\n${stack(undo)}\n]`);
    if (redo > 0) members.push(`"redo":[\n${stack(redo)}\n]`);
    parts.push(`"history":{${members.join(',')}},`);
  }
  parts.push('"end":true}');
  const file = path.join(dir, 'synthetic.stalx');
  fs.writeFileSync(file, parts.join('\n') + '\n');
  return file;
}

/** A file that starts like a legacy SQLite `.stalx` (the header only). */
function makeLegacy(dir: string): string {
  const file = path.join(dir, 'legacy.stalx');
  fs.writeFileSync(file, Buffer.concat([Buffer.from('SQLite format 3\u0000', 'latin1'), Buffer.alloc(84)]));
  return file;
}

function tempDir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-undo-strip-'));
}

describe('stripUndoHistory', () => {
  it('removes the history member of a v2 file and reports what it held', () => {
    const file = makeV2(tempDir());
    const before = fs.readFileSync(file, 'utf8');

    const result = stripUndoHistory(file);

    expect(result).toEqual({ format: 'v2', cleared: { undo: 3, redo: 2 }, missing: [] });
    const after = fs.readFileSync(file, 'utf8');
    const parsed = JSON.parse(after) as Record<string, unknown>;
    expect(parsed.history).toBeUndefined();
    expect(parsed.end).toBe(true);
    // Every other byte is kept: the result is the file without that one record.
    expect(after).toBe(before.replace(/\n"history":[\s\S]*?\},\n(?="end":true\}\n$)/, '\n'));
  });

  it('leaves the survey alone, even text that looks like the history member', () => {
    const file = makeV2(tempDir());

    stripUndoHistory(file);

    const parsed = JSON.parse(fs.readFileSync(file, 'utf8')) as {
      project: { name: string };
      stations: unknown[];
    };
    expect(parsed.project.name).toBe('Synthetic, with a \n"history": in its name');
    expect(parsed.stations).toEqual([[1, 'A'], [2, 'B']]);
  });

  it('reports an empty stack as missing', () => {
    const file = makeV2(tempDir(), { undo: 2, redo: 0 });

    expect(stripUndoHistory(file)).toEqual({ format: 'v2', cleared: { undo: 2 }, missing: ['redo'] });
  });

  it('is idempotent', () => {
    const file = makeV2(tempDir());

    stripUndoHistory(file);
    const once = fs.readFileSync(file, 'utf8');
    const second = stripUndoHistory(file);

    expect(second).toEqual({ format: 'v2', cleared: {}, missing: ['undo', 'redo'] });
    expect(fs.readFileSync(file, 'utf8')).toBe(once);
  });

  it('handles a v2 file with no history at all', () => {
    const file = makeV2(tempDir(), { undo: 0, redo: 0 });
    const before = fs.readFileSync(file, 'utf8');

    expect(stripUndoHistory(file)).toEqual({ format: 'v2', cleared: {}, missing: ['undo', 'redo'] });
    expect(fs.readFileSync(file, 'utf8')).toBe(before);
  });

  it('leaves a legacy file untouched: the app drops its history on open', () => {
    const file = makeLegacy(tempDir());
    const before = fs.readFileSync(file);

    expect(stripUndoHistory(file)).toEqual({ format: 'legacy', cleared: {}, missing: ['undo', 'redo'] });
    expect(fs.readFileSync(file).equals(before)).toBe(true);
  });

  it('sniffs both formats and refuses anything else', () => {
    const dir = tempDir();
    expect(sniffStalx(makeV2(dir))).toBe('v2');
    expect(sniffStalx(makeLegacy(dir))).toBe('legacy');
    const other = path.join(dir, 'other.stalx');
    fs.writeFileSync(other, '{"format":"other"}');
    expect(() => sniffStalx(other)).toThrow(/not a Stalagmight project file/);
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
