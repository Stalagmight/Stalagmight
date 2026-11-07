import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { DEFAULT_FIXTURE } from './fixtures';
import { allowKnownBugs, dumpFullHistory, fuzzFixture, replayTracePath, traceOutPath } from './fuzzKnobs';

// The env names are spelled out literally on purpose: they are a contract with
// `scenario-run.ts`, `fuzz-shrink.ts`, `fuzz-matrix.ts`, every recorded trace's
// `env` block and every repro line in the scenario headers. A rename here must
// fail a test, not just compile.
const KNOBS = [
  'STALAGMIGHT_FUZZ_REPLAY',
  'STALAGMIGHT_FUZZ_TRACE',
  'STALAGMIGHT_FUZZ_FIXTURE',
  'STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS',
  'STALAGMIGHT_FUZZ_DUMP_HISTORY',
] as const;

let saved: Record<string, string | undefined> = {};

beforeEach(() => {
  saved = Object.fromEntries(KNOBS.map((key) => [key, process.env[key]]));
  for (const key of KNOBS) delete process.env[key];
});

afterEach(() => {
  for (const key of KNOBS) {
    const value = saved[key];
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
});

describe('fuzz env knobs', () => {
  it('default when unset: no replay, no forced trace, the default fixture, both switches off', () => {
    expect(replayTracePath()).toBe('');
    expect(traceOutPath()).toBe('');
    expect(fuzzFixture()).toBe(DEFAULT_FIXTURE);
    expect(fuzzFixture()).toBe('fractal_plant_iter3.stalx');
    expect(allowKnownBugs()).toBe(false);
    expect(dumpFullHistory()).toBe(false);
  });

  it('reads each knob from its own variable', () => {
    process.env.STALAGMIGHT_FUZZ_REPLAY = '/traces/recorded.json';
    process.env.STALAGMIGHT_FUZZ_TRACE = '/traces/out.json';
    process.env.STALAGMIGHT_FUZZ_FIXTURE = 'naharon.stalx';
    process.env.STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS = '1';
    process.env.STALAGMIGHT_FUZZ_DUMP_HISTORY = '1';

    expect(replayTracePath()).toBe('/traces/recorded.json');
    expect(traceOutPath()).toBe('/traces/out.json');
    expect(fuzzFixture()).toBe('naharon.stalx');
    expect(allowKnownBugs()).toBe(true);
    expect(dumpFullHistory()).toBe(true);
  });

  it('reads at CALL time, so a runner that sets the env after import is honoured', () => {
    // What `pnpm agent:scenario --trace` and `pnpm agent:fuzz-shrink` do: the
    // scenario modules are already loaded when `main()` sets these.
    expect(replayTracePath()).toBe('');
    expect(fuzzFixture()).toBe(DEFAULT_FIXTURE);

    process.env.STALAGMIGHT_FUZZ_REPLAY = '/traces/late.json';
    process.env.STALAGMIGHT_FUZZ_FIXTURE = 'naharon.stalx';

    expect(replayTracePath()).toBe('/traces/late.json');
    expect(fuzzFixture()).toBe('naharon.stalx');

    delete process.env.STALAGMIGHT_FUZZ_REPLAY;
    expect(replayTracePath()).toBe('');
  });

  it('an EMPTY fixture variable falls back to the default; empty trace paths stay empty', () => {
    process.env.STALAGMIGHT_FUZZ_FIXTURE = '';
    process.env.STALAGMIGHT_FUZZ_REPLAY = '';
    process.env.STALAGMIGHT_FUZZ_TRACE = '';

    expect(fuzzFixture()).toBe(DEFAULT_FIXTURE);
    expect(replayTracePath()).toBe('');
    expect(traceOutPath()).toBe('');
  });

  it('the two switches accept exactly "1" and nothing else', () => {
    for (const value of ['0', 'true', 'yes', '', ' 1', '01']) {
      process.env.STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS = value;
      process.env.STALAGMIGHT_FUZZ_DUMP_HISTORY = value;
      expect(allowKnownBugs(), JSON.stringify(value)).toBe(false);
      expect(dumpFullHistory(), JSON.stringify(value)).toBe(false);
    }
  });
});
