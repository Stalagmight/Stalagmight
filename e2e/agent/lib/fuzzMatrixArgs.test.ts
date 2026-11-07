/**
 * The matrix's argument contract, and the one property a comment cannot hold:
 * the axis refusal list is the SAME list the per-cell environment writes.
 *
 * `fuzz-matrix.ts` itself is not importable from a test - it calls `main()` at
 * module scope, so importing it would launch a matrix - which is why the parsing
 * lives in `fuzzMatrixArgs.ts` and this file tests that.
 */
import { describe, expect, it } from 'vitest';

import {
  BASE_PORT,
  INHERITED_ENV_BLOCKLIST,
  MatrixArgError,
  MAX_WORKERS,
  PORT_STRIDE,
  RESERVED_AXIS_KEYS,
  TOP_DISPLAY,
  cellEnvOverrides,
  parseArgs,
  parseAxis,
  parseSeeds,
  slotEnv,
} from './fuzzMatrixArgs';

/** Every flag list below goes through the real parser, defaults and all. */
function parse(...argv: string[]): ReturnType<typeof parseArgs> {
  return parseArgs(argv);
}

describe('--axis', () => {
  it('sweeps a knob the matrix does not own', () => {
    const axis = parseAxis('STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P=0,0.5,1');
    expect(axis).toEqual([
      { key: 'STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P', value: '0' },
      { key: 'STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P', value: '0.5' },
      { key: 'STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P', value: '1' },
    ]);
  });

  it('keeps an empty value, which is how a cell runs with the knob blank', () => {
    expect(parseAxis('STALAGMIGHT_FUZZ_EXPORT_FORMATS=,csv')).toEqual([
      { key: 'STALAGMIGHT_FUZZ_EXPORT_FORMATS', value: '' },
      { key: 'STALAGMIGHT_FUZZ_EXPORT_FORMATS', value: 'csv' },
    ]);
  });

  it('refuses a non-STALAGMIGHT key, because the cells run cargo and launch the app', () => {
    for (const key of ['PATH', 'NODE_OPTIONS', 'LD_PRELOAD', 'stalagmight_fuzz_x']) {
      expect(() => parseAxis(`${key}=1`)).toThrow(MatrixArgError);
    }
  });

  it('refuses every key the matrix assigns per cell, naming the flag that owns it', () => {
    // The bug: an axis was applied LAST, so `--axis STALAGMIGHT_FUZZ_FIXTURE=x`
    // overrode the per-cell fixture and every row of the report table went on
    // claiming the fixture it was supposed to have run. The port and display
    // keys are worse - they are the worker isolation, so one value puts K
    // concurrent sessions on one port block and one Xvfb display.
    expect(() => parseAxis('STALAGMIGHT_FUZZ_FIXTURE=a.stalx,b.stalx')).toThrow(/--fixtures/);
    expect(() => parseAxis('STALAGMIGHT_AGENT_BASE_PORT=4544')).toThrow(/isolation/);
    expect(() => parseAxis('STALAGMIGHT_AGENT_DISPLAYS=:89')).toThrow(/isolation/);
    expect(() => parseAxis('STALAGMIGHT_AGENT_RELEASE=0,1')).toThrow(/--release/);
    // These two were silently DELETED after the axis spread, which is the same
    // "your flag did nothing" outcome by a different route.
    expect(() => parseAxis('STALAGMIGHT_FUZZ_REPLAY=t.json')).toThrow(/agent:scenario/);
    expect(() => parseAxis('STALAGMIGHT_FUZZ_TRACE=out.json')).toThrow(/agent:scenario/);
    for (const key of RESERVED_AXIS_KEYS) {
      expect(() => parseAxis(`${key}=v`)).toThrow(MatrixArgError);
      // The refusal lists the whole reserved set, so a user sees the boundary
      // rather than one key at a time.
      expect(() => parseAxis(`${key}=v`)).toThrow(/Reserved: /);
    }
  });

  it('is refused through parseArgs too, not just through parseAxis', () => {
    expect(() => parse('--axis', 'STALAGMIGHT_FUZZ_FIXTURE=a.stalx')).toThrow(MatrixArgError);
    expect(() => parse('--axis=STALAGMIGHT_AGENT_DISPLAYS=:88')).toThrow(MatrixArgError);
  });

  it('needs a KEY=VALUES shape', () => {
    for (const raw of ['', 'STALAGMIGHT_FUZZ_X', '=1']) {
      expect(() => parseAxis(raw)).toThrow(MatrixArgError);
    }
  });
});

describe('the reserved set is what the cell environment writes', () => {
  /**
   * The anti-drift property, and the reason `cellEnvOverrides` is a separate
   * function: a future per-cell knob that is not reserved would be silently
   * overridable by `--axis` again, and only this assertion notices.
   */
  it('every key a cell assigns or deletes is refused as an axis', () => {
    for (const release of [false, true]) {
      for (const slot of [0, 1, MAX_WORKERS - 1]) {
        const { set, unset } = cellEnvOverrides(slot, 'probe.stalx', release);
        for (const key of [...Object.keys(set), ...unset]) {
          expect(RESERVED_AXIS_KEYS).toContain(key);
        }
      }
    }
  });

  it('reserves nothing it does not write, over the union of both release modes', () => {
    const written = new Set<string>(INHERITED_ENV_BLOCKLIST);
    for (const release of [false, true]) {
      for (const key of Object.keys(cellEnvOverrides(0, 'probe.stalx', release).set)) written.add(key);
    }
    expect([...written].sort()).toEqual([...RESERVED_AXIS_KEYS].sort());
  });
});

describe('slotEnv', () => {
  it('gives each slot its own port block and display', () => {
    const a = slotEnv(0, 'a.stalx', false, null);
    const b = slotEnv(1, 'b.stalx', false, null);
    expect(a.STALAGMIGHT_AGENT_BASE_PORT).toBe(String(BASE_PORT));
    expect(b.STALAGMIGHT_AGENT_BASE_PORT).toBe(String(BASE_PORT + PORT_STRIDE));
    expect(a.STALAGMIGHT_AGENT_DISPLAYS).toBe(`:${TOP_DISPLAY}`);
    expect(b.STALAGMIGHT_AGENT_DISPLAYS).toBe(`:${TOP_DISPLAY - 1}`);
    expect(a.STALAGMIGHT_FUZZ_FIXTURE).toBe('a.stalx');
  });

  it('drops the replay knobs however they got here', () => {
    const saved = {
      replay: process.env.STALAGMIGHT_FUZZ_REPLAY,
      trace: process.env.STALAGMIGHT_FUZZ_TRACE,
    };
    try {
      process.env.STALAGMIGHT_FUZZ_REPLAY = '/tmp/t.json';
      process.env.STALAGMIGHT_FUZZ_TRACE = '/tmp/out.json';
      const env = slotEnv(0, 'a.stalx', false, null);
      expect(env.STALAGMIGHT_FUZZ_REPLAY).toBeUndefined();
      expect(env.STALAGMIGHT_FUZZ_TRACE).toBeUndefined();
    } finally {
      if (saved.replay === undefined) delete process.env.STALAGMIGHT_FUZZ_REPLAY;
      else process.env.STALAGMIGHT_FUZZ_REPLAY = saved.replay;
      if (saved.trace === undefined) delete process.env.STALAGMIGHT_FUZZ_TRACE;
      else process.env.STALAGMIGHT_FUZZ_TRACE = saved.trace;
    }
  });

  it('lets an allowed axis beat the INHERITED environment, which is the ordering that survives', () => {
    const key = 'STALAGMIGHT_FUZZ_EXPORT_MOVES';
    const saved = process.env[key];
    try {
      process.env[key] = '99';
      const env = slotEnv(0, 'a.stalx', false, { key, value: '3' });
      expect(env[key]).toBe('3');
    } finally {
      if (saved === undefined) delete process.env[key];
      else process.env[key] = saved;
    }
  });

  it('sets the release flag only when asked', () => {
    const saved = process.env.STALAGMIGHT_AGENT_RELEASE;
    try {
      delete process.env.STALAGMIGHT_AGENT_RELEASE;
      expect(slotEnv(0, 'a.stalx', false, null).STALAGMIGHT_AGENT_RELEASE).toBeUndefined();
      expect(slotEnv(0, 'a.stalx', true, null).STALAGMIGHT_AGENT_RELEASE).toBe('1');
    } finally {
      if (saved === undefined) delete process.env.STALAGMIGHT_AGENT_RELEASE;
      else process.env.STALAGMIGHT_AGENT_RELEASE = saved;
    }
  });
});

describe('the other flags still parse as they did', () => {
  it('reads seed ranges and lists', () => {
    expect(parseSeeds('1..4')).toEqual([1, 2, 3, 4]);
    expect(parseSeeds('1,3,7')).toEqual([1, 3, 7]);
    expect(parseSeeds('-2..0')).toEqual([-2, -1, 0]);
    expect(() => parseSeeds('4..1')).toThrow(MatrixArgError);
    expect(() => parseSeeds('1..2000')).toThrow(MatrixArgError);
    expect(() => parseSeeds('a')).toThrow(MatrixArgError);
  });

  it('defaults the scenario and validates its shape', () => {
    expect(parse().scenario).toBe('undo-redo-fuzz');
    expect(parse('--scenario', 'export-roundtrip-fuzz').scenario).toBe('export-roundtrip-fuzz');
    expect(() => parse('--scenario', 'Export_Fuzz')).toThrow(MatrixArgError);
  });

  it('treats --release as a switch but honours an explicit 0', () => {
    expect(parse('--release').release).toBe(true);
    expect(parse('--release=0').release).toBe(false);
    expect(() => parse('--release=maybe')).toThrow(MatrixArgError);
  });

  it('clamps --workers to the display pool and says so in a note', () => {
    const opts = parse('--workers', String(MAX_WORKERS + 3));
    expect(opts.workers).toBe(MAX_WORKERS);
    expect(opts.notes.join('\n')).toMatch(/clamped/);
    expect(parse('--workers', '2').notes).toEqual([]);
  });

  it('refuses an unknown flag and a bare argument', () => {
    expect(() => parse('--nope', '1')).toThrow(MatrixArgError);
    expect(() => parse('positional')).toThrow(MatrixArgError);
  });

  it('prints what the user typed as its length, never its text (#576 W51)', () => {
    const refusal = (...argv: string[]): string => {
      try {
        parse(...argv);
      } catch (err) {
        return (err as Error).message;
      }
      return '';
    };
    const refusals = [
      refusal('/home/qzCaveA/survey.tml'),
      refusal('--qzCaveA'),
      refusal('--seeds', '1,qzCaveA'),
      refusal('--workers', 'qzCaveA'),
      refusal('--release=qzCaveA'),
      refusal('--axis', 'qzCaveA'),
      refusal('--axis', 'qzCaveA=1'),
      refusal('--fixtures', 'qzCaveA/x.stalx'),
      refusal('--fixtures', 'qzCaveA.tml'),
      refusal('--fixtures', 'qzCaveA.stalx'),
      refusal('--scenario', 'qzCaveA'),
    ];
    for (const message of refusals) {
      expect(message).not.toBe('');
      expect(message).not.toContain('qzCaveA');
    }
    expect(refusals[0]).toBe('unexpected argument, a value of 24 character(s)');
    expect(refusal('--workers', '0')).toBe('--workers needs a positive integer, got 0');
  });

  it('takes fixture BASENAMES that exist', () => {
    // Whatever the tracked tree actually holds - naming one here would break the
    // day somebody adds or renames a fixture, which is not what this asserts.
    const known = parse().fixtures[0];
    expect(known).toMatch(/\.stalx$/);
    expect(parse('--fixtures', known!).fixtures).toEqual([known]); // toMatch above: a string
    expect(() => parse('--fixtures', `sub/${known}`)).toThrow(MatrixArgError);
    expect(() => parse('--fixtures', 'anything.tml')).toThrow(MatrixArgError);
    expect(() => parse('--fixtures', 'no-such-fixture.stalx')).toThrow(MatrixArgError);
  });
});
