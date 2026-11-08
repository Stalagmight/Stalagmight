/**
 * #576 W58 Q9: a fuzz trace records, restores and prints a knob's value only
 * when the knob is registered (`traceKnobs.ts`) and the value has its shape.
 * Synthetic values only (`qz...`).
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { DEFAULT_FIXTURE } from './fixtures';
import { NEVER_RESTORED, TraceRecorder, readTrace, restoreTraceEnv, writeTrace } from './fuzzTrace';
import { fixtureTag } from './privateFixtures';
import { TRACE_KNOBS, knobValueShaped, shareableKnobValue } from './traceKnobs';

const AGENT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const OWNED = [
  'STALAGMIGHT_FUZZ_STREAM',
  'STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P',
  'STALAGMIGHT_FUZZ_REPAIR',
  'STALAGMIGHT_FUZZ_EXPORT_FORMATS',
  'STALAGMIGHT_FUZZ_QZ_UNREGISTERED',
  'STALAGMIGHT_FUZZ_DUMP_HISTORY',
];
const saved = new Map<string, string | undefined>();
beforeEach(() => {
  for (const key of OWNED) {
    saved.set(key, process.env[key]);
    delete process.env[key];
  }
});
afterEach(() => {
  for (const key of OWNED) {
    const value = saved.get(key);
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
});

/** Every non-test source file under e2e/agent. */
function sources(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === 'node_modules' ? [] : sources(full);
    return /\.(?:ts|mts|mjs)$/.test(entry.name) && !/\.test\.\w+$/.test(entry.name) ? [full] : [];
  });
}

describe('the trace knob registry (#576 W58 Q9)', () => {
  it('registers every STALAGMIGHT_FUZZ_* / STALAGMIGHT_PERSIST_FUZZ_* knob the harness names, or a path knob no trace carries', () => {
    const named = new Set(
      sources(AGENT).flatMap((file) => [...fs.readFileSync(file, 'utf8').matchAll(/STALAGMIGHT_(?:PERSIST_)?FUZZ_[A-Z0-9_]*[A-Z0-9]/g)].map((m) => m[0])),
    );
    expect(named.size).toBeGreaterThan(20);
    expect([...named].filter((key) => !(key in TRACE_KNOBS) && !NEVER_RESTORED.includes(key)).sort()).toEqual([]);
  });

  it("accepts each knob's values and refuses text", () => {
    expect(knobValueShaped('STALAGMIGHT_FUZZ_STREAM', 'v3')).toBe(true);
    expect(knobValueShaped('STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P', '0.25')).toBe(true);
    expect(knobValueShaped('STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P', '')).toBe(true);
    expect(knobValueShaped('STALAGMIGHT_FUZZ_REPAIR', '1')).toBe(true);
    expect(knobValueShaped('STALAGMIGHT_FUZZ_EXPORT_FORMATS', 'csv, tmlu,diveProfile')).toBe(true);
    expect(knobValueShaped('STALAGMIGHT_FUZZ_IMPORT_FORMAT', 'DMP,gpx')).toBe(true);
    expect(knobValueShaped('STALAGMIGHT_FUZZ_FIXTURE', DEFAULT_FIXTURE)).toBe(true);
    for (const [key, value] of [
      ['STALAGMIGHT_FUZZ_REPAIR', 'qz Cave A'],
      ['STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P', 'qzCaveA'],
      ['STALAGMIGHT_FUZZ_EXPORT_FORMATS', 'csv,qzcavea'],
      ['STALAGMIGHT_FUZZ_STREAM', 'v3\n# qz'],
      ['STALAGMIGHT_FUZZ_FIXTURE', 'qz/CaveA.stalx'],
      ['STALAGMIGHT_FUZZ_QZ_UNREGISTERED', '1'],
    ] as const) {
      expect(knobValueShaped(key, value), `${key}=${value}`).toBe(false);
    }
  });

  it('records a knob of its shape and only the length of any other value', () => {
    process.env.STALAGMIGHT_FUZZ_STREAM = 'v3';
    process.env.STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P = '0.25';
    process.env.STALAGMIGHT_FUZZ_REPAIR = 'qz Cave A';
    process.env.STALAGMIGHT_FUZZ_QZ_UNREGISTERED = 'qzCaveB';
    const trace = new TraceRecorder().build({
      scenario: 'undo-redo-fuzz',
      seed: 1,
      iterations: 1,
      fixture: DEFAULT_FIXTURE,
      stream: 'v3',
      failure: null,
    });
    expect(trace.env).toMatchObject({ STALAGMIGHT_FUZZ_STREAM: 'v3', STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P: '0.25' });
    expect(trace.env).not.toHaveProperty('STALAGMIGHT_FUZZ_REPAIR');
    expect(trace.envWithheld).toEqual(
      expect.objectContaining({ STALAGMIGHT_FUZZ_REPAIR: 9, STALAGMIGHT_FUZZ_QZ_UNREGISTERED: 7 }),
    );
    expect(JSON.stringify(trace)).not.toMatch(/qz Cave A|qzCaveB/);

    // It survives the round trip through a file.
    const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'qz-knobs-')), 'trace.json');
    writeTrace(file, trace);
    expect(readTrace(file).envWithheld).toEqual(trace.envWithheld);
  });

  it('restores only a registered knob of its shape, and leaves a withheld one as the shell has it', () => {
    process.env.STALAGMIGHT_FUZZ_DUMP_HISTORY = '1';
    const { applied, refused, cleared, withheld } = restoreTraceEnv(
      { STALAGMIGHT_FUZZ_STREAM: 'v3', STALAGMIGHT_FUZZ_REPAIR: 'qzCaveA', STALAGMIGHT_FUZZ_QZ_UNREGISTERED: '1' },
      { STALAGMIGHT_FUZZ_DUMP_HISTORY: 4 },
    );
    expect(applied).toEqual([['STALAGMIGHT_FUZZ_STREAM', 'v3']]);
    expect(refused).toEqual(['STALAGMIGHT_FUZZ_REPAIR', 'STALAGMIGHT_FUZZ_QZ_UNREGISTERED']);
    expect(process.env.STALAGMIGHT_FUZZ_REPAIR).toBeUndefined();
    expect(withheld).toEqual(['STALAGMIGHT_FUZZ_DUMP_HISTORY']);
    expect(cleared).not.toContain('STALAGMIGHT_FUZZ_DUMP_HISTORY');
    expect(process.env.STALAGMIGHT_FUZZ_DUMP_HISTORY).toBe('1');
  });

  it('prints a knob value of its shape as itself, the fixture by its tracked name or tag, anything else as its length', () => {
    expect(shareableKnobValue('STALAGMIGHT_FUZZ_STREAM', 'v3')).toBe('v3');
    expect(shareableKnobValue('STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P', '')).toBe('""');
    expect(shareableKnobValue('STALAGMIGHT_FUZZ_REPAIR', 'qzCaveA')).toBe('a string of 7 character(s)');
    expect(shareableKnobValue('STALAGMIGHT_FUZZ_QZ_UNREGISTERED', '1')).toBe('a string of 1 character(s)');
    expect(shareableKnobValue('STALAGMIGHT_FUZZ_FIXTURE', DEFAULT_FIXTURE)).toBe(DEFAULT_FIXTURE);
    expect(shareableKnobValue('STALAGMIGHT_FUZZ_FIXTURE', 'qzCaveA.stalx')).toBe(fixtureTag('qzCaveA.stalx'));
  });

  it('refuses a trace whose envWithheld is not knob names to lengths', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qz-knobs-'));
    const file = path.join(dir, 'trace.json');
    const trace = new TraceRecorder().build({
      scenario: 'undo-redo-fuzz',
      seed: 1,
      iterations: 1,
      fixture: DEFAULT_FIXTURE,
      stream: 'v3',
      failure: null,
    });
    fs.writeFileSync(file, JSON.stringify({ ...trace, envWithheld: { STALAGMIGHT_FUZZ_REPAIR: 'qzCaveA' } }));
    expect(() => readTrace(file)).toThrow(/has envWithheld an object; must map knob names to lengths/);
  });
});
