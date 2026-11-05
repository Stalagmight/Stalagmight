import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import * as zlib from 'zlib';
import { createRequire } from 'node:module';
import type { DatabaseSync as DatabaseSyncType } from 'node:sqlite';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import type { ScenarioContext, ScenarioMetrics } from '../scenarios/framework';
import { compareRecordStreams } from './fuzzDeterminism';
import { DetProbe, detProbe } from './fuzzDeterminismProbe';
import { runDeterminismPair, type DeterminismPairMeta } from './fuzzDeterminismRun';
import { fuzzDeterminism, fuzzDeterminismPlant } from './fuzzKnobs';

const nodeRequire = createRequire(import.meta.url);
const { DatabaseSync } = nodeRequire('node:sqlite') as { DatabaseSync: typeof DatabaseSyncType };

let dir = '';
beforeEach(() => {
  dir = fs.mkdtempSync(path.join(os.tmpdir(), 'fzdet-probe-test-'));
});
afterEach(() => {
  fs.rmSync(dir, { recursive: true, force: true });
});

/** A synthetic .stalx-shaped database: a stamped row, a gzip undo payload, a text-keyed table. */
function writeStalx(file: string, stamp: string, shotLength: number, settingsOrder: [string, string][]): void {
  const db = new DatabaseSync(file);
  db.exec(`
    CREATE TABLE station_merges (id INTEGER PRIMARY KEY, a INTEGER NOT NULL, merged_at TEXT NOT NULL);
    CREATE TABLE shots (id INTEGER PRIMARY KEY, length REAL NOT NULL, uuid TEXT);
    CREATE TABLE undo_stack (id INTEGER PRIMARY KEY, sequence INTEGER, undo_data_json BLOB, executed_at TEXT);
    CREATE TABLE display_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
  `);
  db.prepare('INSERT INTO station_merges VALUES (1, 7, ?)').run(stamp);
  db.prepare('INSERT INTO shots VALUES (10, ?, ?)').run(shotLength, `${stamp.slice(0, 4)}0000-0000-4000-8000-000000000000`);
  const payload = zlib.gzipSync(Buffer.from(JSON.stringify({ Merge: { merged_at: stamp, a: 7 } })));
  db.prepare('INSERT INTO undo_stack VALUES (1, 0, ?, ?)').run(payload, stamp);
  // Inserted in a per-replay order: rows are compared in primary-key order.
  for (const [key, value] of settingsOrder) db.prepare('INSERT INTO display_settings VALUES (?, ?)').run(key, value);
  db.close();
}

function fakeCtx(state: () => unknown): ScenarioContext {
  return {
    eval: async () => JSON.stringify(state()),
    log: () => undefined,
  } as unknown as ScenarioContext;
}

describe('DetProbe', () => {
  it('reads a saved .stalx row by row: stamps, uuids and insertion order are not differences', () => {
    const a = path.join(dir, 'a.stalx');
    const b = path.join(dir, 'b.stalx');
    writeStalx(a, '2026-11-05T10:00:00Z', 4.5, [
      ['k1', 'v1'],
      ['k2', 'v2'],
    ]);
    writeStalx(b, '2027-01-01T00:00:00Z', 4.5, [
      ['k2', 'v2'],
      ['k1', 'v1'],
    ]);
    const left = new DetProbe('A');
    const right = new DetProbe('B');
    left.stalx('saved', a);
    right.stalx('saved', b);
    expect(left.records.map((record) => record.label)).toEqual([
      'saved: schema',
      'saved: table display_settings',
      'saved: table shots',
      'saved: table station_merges',
      'saved: table undo_stack',
    ]);
    expect(compareRecordStreams(left.records, right.records)).toBeNull();
  });

  it('catches a planted difference in one saved row', () => {
    const a = path.join(dir, 'a.stalx');
    const b = path.join(dir, 'b.stalx');
    writeStalx(a, '2026-11-05T10:00:00Z', 4.5, [['k1', 'v1']]);
    writeStalx(b, '2026-11-05T10:00:00Z', 4.75, [['k1', 'v1']]);
    const left = new DetProbe('A');
    const right = new DetProbe('B');
    left.stalx('saved', a);
    right.stalx('saved', b);
    expect(compareRecordStreams(left.records, right.records)).toMatchObject({
      label: 'saved: table shots',
      path: '$.rows[0].length',
      a: '4.5',
      b: '4.75',
    });
  });

  it('records the state after a fused step with the page timings and the canonical length masked', async () => {
    const run = async (probe: DetProbe, actionMs: number): Promise<void> => {
      const ctx = fakeCtx(() => ({ project: { ok: { modified_at: String(actionMs), sections: [] } } }));
      // `bytes` follows the stamps' length, as `modified_at` here does.
      await probe.step(ctx, { kind: 'undo', action: 'undo' }, { undoLen: 1, bytes: 100 + actionMs, timing: { actionMs } });
    };
    const left = new DetProbe('A');
    const right = new DetProbe('B');
    await run(left, 3);
    await run(right, 9);
    expect(left.records.map((record) => record.kind)).toEqual(['answer', 'state']);
    expect(compareRecordStreams(left.records, right.records)).toBeNull();
  });

  it('records a missing export file as such rather than throwing', () => {
    const probe = new DetProbe('A');
    probe.file('export move 1 file', path.join(dir, 'never-written.csv'));
    expect(probe.records).toHaveLength(1);
  });
});

describe('runDeterminismPair', () => {
  const meta: DeterminismPairMeta = {
    scenario: 'undo-redo-fuzz',
    seed: 5,
    iterations: 3,
    fixture: 'synthetic.stalx',
    mode: 'one-session',
    plantAt: null,
  };
  const metrics = (): ScenarioMetrics => ({ scenario: meta.scenario, seed: 5, iterations: 3, durationMs: 1, metrics: {} });

  /** A fake scenario: two steps, each recording the action and the state. */
  const scenario = (values: { A: number[]; B: number[] }, fail?: 'A' | 'B') => async (replay: 'A' | 'B') => {
    const probe = detProbe();
    if (probe === null) throw new Error('no probe installed');
    for (const [index, value] of values[replay].entries()) {
      probe.action({ kind: 'mutate', index });
      await probe.state(fakeCtx(() => ({ project: { ok: { length: value } } })), `after step ${index + 1}`);
    }
    if (fail === replay) throw new Error(`model mismatch at depth 1 in replay ${replay}`);
    return metrics();
  };

  it('passes when both replays agree, and uninstalls the probe', async () => {
    const out = await runDeterminismPair(meta, scenario({ A: [1, 2], B: [1, 2] }));
    expect(out.metrics['determinism:records']).toBe(4);
    expect(detProbe()).toBeNull();
  });

  it('fails with the seed, the trace step and the first differing path', async () => {
    await expect(runDeterminismPair(meta, scenario({ A: [1, 2], B: [1, 3] }))).rejects.toThrow(
      /seed 5 diverged at trace step 2 \(record #4, state "after step 2"\)\n {2}first differing path: \$\.project\.ok\.length/,
    );
  });

  it('the plant knob makes replay B differ at its Nth state record', async () => {
    await expect(runDeterminismPair({ ...meta, plantAt: 2 }, scenario({ A: [1.5, 2.5], B: [1.5, 2.5] }))).rejects.toThrow(
      /trace step 2 .*\n {2}first differing path: \$\.project\.ok\.length/,
    );
  });

  it('a failure in one replay only is a determinism failure', async () => {
    await expect(runDeterminismPair(meta, scenario({ A: [1, 2], B: [1, 2] }, 'B'))).rejects.toThrow(
      /replay B of seed 5 failed and replay A passed/,
    );
  });
});

describe('knobs', () => {
  const saved = { mode: process.env.STALAGMIGHT_FUZZ_DETERMINISM, plant: process.env.STALAGMIGHT_FUZZ_DETERMINISM_PLANT };
  afterEach(() => {
    for (const [key, value] of [
      ['STALAGMIGHT_FUZZ_DETERMINISM', saved.mode],
      ['STALAGMIGHT_FUZZ_DETERMINISM_PLANT', saved.plant],
    ] as const) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  });

  it('parses the mode and refuses a typo', () => {
    delete process.env.STALAGMIGHT_FUZZ_DETERMINISM;
    expect(fuzzDeterminism()).toBe('off');
    process.env.STALAGMIGHT_FUZZ_DETERMINISM = '1';
    expect(fuzzDeterminism()).toBe('one-session');
    process.env.STALAGMIGHT_FUZZ_DETERMINISM = '2sessions';
    expect(fuzzDeterminism()).toBe('two-sessions');
    process.env.STALAGMIGHT_FUZZ_DETERMINISM = 'yes';
    expect(() => fuzzDeterminism()).toThrow(/expected 1/);
  });

  it('parses the plant knob', () => {
    delete process.env.STALAGMIGHT_FUZZ_DETERMINISM_PLANT;
    expect(fuzzDeterminismPlant()).toBeNull();
    process.env.STALAGMIGHT_FUZZ_DETERMINISM_PLANT = '3';
    expect(fuzzDeterminismPlant()).toBe(3);
    process.env.STALAGMIGHT_FUZZ_DETERMINISM_PLANT = '0';
    expect(() => fuzzDeterminismPlant()).toThrow(/positive integer/);
  });
});
