/**
 * Argument parsing and per-cell environment for `fuzz-matrix.ts`.
 *
 * Split out of the runner for one reason: the runner calls `main()` at module
 * scope, so importing it from a test would launch a matrix. Everything here is
 * pure - it decides, it never spawns - which is what lets
 * `fuzzMatrixArgs.test.ts` pin the two properties that used to be enforced only
 * by reading the file:
 *
 *   - `--axis` may not name a variable the matrix assigns itself, and
 *   - the reserved list is the SAME list `cellEnvOverrides` writes, so adding a
 *     per-cell knob without reserving it fails a test instead of silently
 *     becoming overridable.
 *
 * `bail`-style `process.exit` lives in the runner; here a bad argument throws
 * `MatrixArgError`, which the runner turns back into "message, usage, exit 2".
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

import { fixtureSourcePath } from './fixtures';
import { cliShaped, foldLocalPaths } from './shareableFailure';

/** A bad flag. The runner prints `.message`, then the usage line, then exits 2. */
export class MatrixArgError extends Error {}

function bad(message: string): never {
  throw new MatrixArgError(message);
}

/**
 * A value the user typed that is not what the flag takes, as the refusal prints
 * it: its length (#576 W51). The refusal goes to stderr, which people paste, and
 * a stray argument can be a path or a name. A token shaped like a flag or a
 * number prints as itself, quoted (#576 W58 Q4): neither shape can be either.
 */
function typed(raw: string): string {
  return cliShaped(raw) ? JSON.stringify(raw) : `a value of ${raw.length} character(s)`;
}

/** Size of session.ts's Xvfb display pool - one display per worker, so 5 is the ceiling. */
export const MAX_WORKERS = 5;
export const BASE_PORT = 4544;
export const PORT_STRIDE = 20;
export const TOP_DISPLAY = 89;
export const DEFAULT_ITERATIONS = 60;
export const DEFAULT_SCENARIO = 'undo-redo-fuzz';

/** Env keys `--axis` may set at all. See fuzz-matrix.ts's header for why. */
const AXIS_KEY_RE = /^STALAGMIGHT_[A-Z0-9_]{1,64}$/;

/**
 * Replay knobs: DROPPED from every cell, not inherited.
 *
 * A cell GENERATES moves at its own seed, so an exported STALAGMIGHT_FUZZ_REPLAY
 * left over from triage would make every cell replay that one trace - a full
 * pass/fail table and merged metrics for coverage nothing exercised - and an
 * exported STALAGMIGHT_FUZZ_TRACE would point all K concurrent workers at one
 * output file to clobber in turn.
 */
export const INHERITED_ENV_BLOCKLIST = ['STALAGMIGHT_FUZZ_REPLAY', 'STALAGMIGHT_FUZZ_TRACE'] as const;

/**
 * Every environment variable the MATRIX ITSELF decides per cell, and therefore
 * every key `--axis` is refused.
 *
 * An axis is applied to the child environment, and it used to be applied LAST so
 * that it beat whatever the invoking shell had exported. That ordering is right
 * for a knob the matrix does not own and wrong for one it does: `--axis
 * STALAGMIGHT_FUZZ_FIXTURE=a.stalx` overrode the per-cell fixture assignment, so
 * every cell ran the same fixture while the report table went on labelling them
 * by the `--fixtures` value they were supposed to have. `--axis
 * STALAGMIGHT_AGENT_BASE_PORT` or `..._DISPLAYS` was worse: those are the worker
 * ISOLATION, so pinning one value puts K concurrent sessions on one port block
 * and one Xvfb display, which is the race `slotEnv` exists to remove.
 *
 * Refusing beats reordering. Reordering would silently ignore what the user
 * asked for; a refusal names the key and says which flag owns it.
 *
 * The two blocklist entries are here too. They were already dropped after the
 * axis spread, so an axis naming one was silently discarded - the same "your
 * flag did nothing" failure, and worth the same message.
 */
export const RESERVED_AXIS_KEYS: readonly string[] = [
  'STALAGMIGHT_FUZZ_FIXTURE',
  'STALAGMIGHT_AGENT_BASE_PORT',
  'STALAGMIGHT_AGENT_DISPLAYS',
  'STALAGMIGHT_AGENT_RELEASE',
  ...INHERITED_ENV_BLOCKLIST,
];

/** Which flag owns a reserved key, for the refusal message. */
const RESERVED_BY: Readonly<Record<string, string>> = {
  STALAGMIGHT_FUZZ_FIXTURE: '--fixtures',
  STALAGMIGHT_AGENT_BASE_PORT: 'worker isolation (one port block per slot)',
  STALAGMIGHT_AGENT_DISPLAYS: 'worker isolation (one Xvfb display per slot)',
  STALAGMIGHT_AGENT_RELEASE: '--release',
  STALAGMIGHT_FUZZ_REPLAY: 'pnpm agent:scenario --trace (a matrix generates its own moves)',
  STALAGMIGHT_FUZZ_TRACE: 'pnpm agent:scenario --trace (K workers cannot share one output file)',
};

/** One value of the optional third dimension. `null` = no axis was given. */
export interface AxisValue {
  key: string;
  value: string;
}

export interface Options {
  scenario: string;
  fixtures: string[];
  seeds: number[];
  /** `[null]` when no `--axis` was passed, so the cell product is unchanged. */
  axis: (AxisValue | null)[];
  iterations: number;
  workers: number;
  /** Drive the release binary (STALAGMIGHT_AGENT_RELEASE=1 in every child). */
  release: boolean;
  /** Advisory lines the runner prints before it starts (e.g. a clamped --workers). */
  notes: string[];
}

export function parseSeeds(raw: string): number[] {
  const range = raw.match(/^(-?\d+)\.\.(-?\d+)$/);
  if (range) {
    const from = Number(range[1]);
    const to = Number(range[2]);
    if (to < from) bad(`--seeds "${raw}" is an empty range (end < start)`);
    if (to - from > 999) bad(`--seeds "${raw}" spans more than 1000 seeds`);
    return Array.from({ length: to - from + 1 }, (_unused, i) => from + i);
  }
  const parts = raw.split(',').map((entry) => entry.trim()).filter(Boolean);
  if (!parts.length) bad('--seeds needs at least one value');
  return parts.map((entry) => {
    if (!/^-?\d+$/.test(entry)) bad(`--seeds: ${typed(entry)} is not an integer`);
    return Number(entry);
  });
}

function positiveInt(flag: string, raw: string): number {
  if (!/^\d+$/.test(raw)) bad(`--${flag} needs a positive integer, got ${typed(raw)}`);
  if (Number(raw) < 1) bad(`--${flag} needs a positive integer, got ${Number(raw)}`);
  return Number(raw);
}

/**
 * A switch that also accepts `--flag=VALUE`. Presence alone is NOT enough:
 * `--release=0` reads as "no release" to everyone who types it, and treating it
 * as true would silently drive the release binary - where the `debug_assert!`s
 * and `debug_assertions`-gated self-checks that are oracles in their own right
 * are compiled out, so the sweep passes on seeds the debug build would fail.
 */
function booleanFlag(flag: string, raw: string): boolean {
  const value = raw.trim().toLowerCase();
  if (['1', 'true', 'yes', 'on'].includes(value)) return true;
  if (['0', 'false', 'no', 'off'].includes(value)) return false;
  bad(`--${flag} takes 1/0, true/false, yes/no or on/off, got ${typed(raw)}`);
}

/**
 * `--axis STALAGMIGHT_KEY=v1,v2,...`. An EMPTY value is legal and meaningful -
 * it is how a cell runs with the knob explicitly unset-ish - so the list is
 * split without filtering blanks.
 */
export function parseAxis(raw: string): AxisValue[] {
  const eq = raw.indexOf('=');
  if (eq <= 0) bad(`--axis needs KEY=v1,v2,..., got ${typed(raw)}`);
  const key = raw.slice(0, eq);
  if (!AXIS_KEY_RE.test(key)) {
    bad(`--axis key ${typed(key)} is not a STALAGMIGHT_* variable; the cells run cargo and launch the app`);
  }
  if (RESERVED_AXIS_KEYS.includes(key)) {
    bad(
      `--axis key "${key}" is assigned by the matrix itself (owner: ${RESERVED_BY[key]}), so an ` +
        `axis over it would either mislabel every cell or collapse the workers onto one port ` +
        `block / display. Reserved: ${[...RESERVED_AXIS_KEYS].join(', ')}`,
    );
  }
  const values = raw.slice(eq + 1).split(',');
  if (values.length === 0) bad('--axis needs at least one value');
  return values.map((value) => ({ key, value }));
}

export function parseArgs(argv: string[]): Options {
  const known = new Set(['scenario', 'fixtures', 'seeds', 'axis', 'workers', 'iterations', 'release']);
  const raw: Record<string, string> = {};
  const notes: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    const token = argv[i]!;
    if (!token.startsWith('--')) bad(`unexpected argument, ${typed(token)}`);
    const eq = token.indexOf('=');
    const flag = eq >= 0 ? token.slice(2, eq) : token.slice(2);
    if (!known.has(flag)) bad(`unknown flag, ${typed(eq >= 0 ? token.slice(0, eq) : token)}`);
    // `--release` is a boolean switch; every other flag takes a value.
    if (flag === 'release' && eq < 0) {
      raw[flag] = '1';
      continue;
    }
    const value = eq >= 0 ? token.slice(eq + 1) : argv[++i];
    if (value === undefined) bad(`--${flag} needs a value`);
    raw[flag] = value;
  }

  let workers = Math.min(4, Math.max(1, os.cpus().length - 2), MAX_WORKERS);
  if (raw.workers !== undefined) {
    workers = positiveInt('workers', raw.workers);
    if (workers > MAX_WORKERS) {
      notes.push(`note: --workers ${workers} clamped to ${MAX_WORKERS}, the size of the Xvfb display pool`);
      workers = MAX_WORKERS;
    }
  }

  // The fixture root stays defined in exactly one place (lib/fixtures.ts).
  const fixtureDir = path.dirname(fixtureSourcePath('probe.stalx'));
  let fixtures: string[];
  if (raw.fixtures === undefined) {
    fixtures = fs.readdirSync(fixtureDir).filter((name) => name.endsWith('.stalx')).sort();
    if (!fixtures.length) bad(`no *.stalx fixtures found in ${foldLocalPaths(fixtureDir)}`);
  } else {
    fixtures = raw.fixtures.split(',').map((entry) => entry.trim()).filter(Boolean);
    if (!fixtures.length) bad('--fixtures needs at least one basename');
    for (const fixture of fixtures) {
      if (fixture !== path.basename(fixture)) bad(`--fixtures takes basenames, got ${typed(fixture)}`);
      if (!fixture.endsWith('.stalx')) bad(`--fixtures: ${typed(fixture)} is not a .stalx`);
      if (!fs.existsSync(fixtureSourcePath(fixture))) bad(`no such fixture in e2e/fixtures/: ${typed(fixture)}`);
    }
  }

  const seeds = raw.seeds === undefined ? [1, 2, 3, 4] : parseSeeds(raw.seeds);
  const iterations = raw.iterations === undefined ? DEFAULT_ITERATIONS : positiveInt('iterations', raw.iterations);
  const release = raw.release === undefined ? false : booleanFlag('release', raw.release);
  // Shape only, never the registry - see fuzz-matrix.ts's header for why
  // importing `./scenarios` in the PARENT would be wrong.
  const scenario = raw.scenario === undefined ? DEFAULT_SCENARIO : raw.scenario;
  if (!/^[a-z][a-z0-9-]{1,63}$/.test(scenario)) {
    bad(`--scenario ${typed(scenario)} is not a scenario name (lower-case, digits and dashes)`);
  }
  const axis: (AxisValue | null)[] = raw.axis === undefined ? [null] : parseAxis(raw.axis);
  return { scenario, fixtures, seeds, axis, iterations, workers, release, notes };
}

/**
 * Everything one cell's environment differs from this process's, and nothing
 * else.
 *
 * Separated from the spread so `RESERVED_AXIS_KEYS` can be checked against what
 * is actually written rather than against a comment. `set` is applied over the
 * inherited environment and `unset` is deleted from it; the union of the two is
 * exactly the reserved set, which `fuzzMatrixArgs.test.ts` asserts.
 *
 * Ports: session.ts does findFreePort(BASE_PORT) over a span of 10, then
 * findFreePort(driverPort + 1000) over another 10. Slot i therefore claims
 * driver ports in [4544+20i, 4544+20i+10) and native ports within
 * [5544+20i, 5562+20i]. Slot i+1's driver window starts 20 above ours and its
 * native window at 5564+20i, so a stride of 20 keeps both windows disjoint.
 *
 * Displays: STALAGMIGHT_AGENT_DISPLAYS narrows the pool to exactly one display.
 * That is load-bearing. The default first-that-starts walk calls a display free
 * when Xvfb has not exited within 250 ms, so two sessions launched together can
 * both conclude they own :89. One display per slot removes the race.
 */
export function cellEnvOverrides(
  slot: number,
  fixture: string,
  release: boolean,
): { set: Record<string, string>; unset: readonly string[] } {
  return {
    set: {
      STALAGMIGHT_FUZZ_FIXTURE: fixture,
      STALAGMIGHT_AGENT_BASE_PORT: String(BASE_PORT + slot * PORT_STRIDE),
      STALAGMIGHT_AGENT_DISPLAYS: `:${TOP_DISPLAY - slot}`,
      // `--release` on the matrix sets it for every child; an inherited
      // STALAGMIGHT_AGENT_RELEASE=1 already reaches them through the spread.
      ...(release ? { STALAGMIGHT_AGENT_RELEASE: '1' } : {}),
    },
    unset: INHERITED_ENV_BLOCKLIST,
  };
}

/**
 * Worker isolation. A SLOT is a lane held by exactly one live child at a time;
 * the next queued cell only takes it after its occupant has exited.
 *
 * The axis is applied BEFORE the matrix's own assignments now, and it makes no
 * difference which way round it goes: `parseAxis` refuses every key
 * `cellEnvOverrides` writes, so the two can no longer name the same variable.
 * The order that remains meaningful is axis-over-INHERITED, which still holds -
 * an axis sweep beats whatever the invoking shell exported for the same key,
 * otherwise half the matrix would silently be one cell.
 */
export function slotEnv(
  slot: number,
  fixture: string,
  release: boolean,
  axis: AxisValue | null,
): NodeJS.ProcessEnv {
  const overrides = cellEnvOverrides(slot, fixture, release);
  const env: NodeJS.ProcessEnv = {
    // Inherited wholesale, so STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS reaches the
    // child exactly as the caller set it.
    ...process.env,
    ...(axis ? { [axis.key]: axis.value } : {}),
    ...overrides.set,
  };
  for (const key of overrides.unset) delete env[key];
  return env;
}
