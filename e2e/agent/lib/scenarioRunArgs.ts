/**
 * `pnpm agent:scenario`'s argument parsing and the run options a replay uses
 * (e2e/agent/scenario-run.ts, which runs `main()` when imported - so the logic
 * that can be tested without the app lives here).
 */
import type { FuzzTrace } from './fuzzTrace';
import { cliValueShape, valueShape } from './shareableFailure';

export interface ScenarioRunArgs {
  name: string;
  seed?: number;
  iterations?: number;
  release: boolean;
  /** Replay a recorded trace instead of drawing moves. */
  trace?: string;
}

export const SCENARIO_RUN_USAGE =
  'usage: pnpm agent:scenario <name> [--seed N] [--iterations M] [--release] [--trace FILE]';

/**
 * Numeric flags, validated exactly as `fuzz-matrix.ts` validates the same two:
 * a seed is any INTEGER (its `--seeds` ranges accept negatives), an iteration
 * count is a POSITIVE one.
 *
 * `Number.isFinite` alone was not enough: it accepts `0`, `-5` and `3.7`, and
 * `runScenario` passes the count straight through, so `--iterations 0` skipped
 * the fuzz loop entirely and still printed PASS - a green verdict for a run that
 * exercised nothing.
 */
export function integerFlag(flag: string, raw: string): number {
  if (!/^-?\d+$/.test(raw)) throw new Error(`flag ${flag} needs an integer, got ${cliValueShape(raw)}`);
  return Number(raw);
}

export function positiveIntFlag(flag: string, raw: string): number {
  if (!/^\d+$/.test(raw)) throw new Error(`flag ${flag} needs a positive integer, got ${cliValueShape(raw)}`);
  if (Number(raw) < 1) throw new Error(`flag ${flag} needs a positive integer, got ${Number(raw)}`);
  return Number(raw);
}

export function parseScenarioArgs(argv: string[]): ScenarioRunArgs {
  const [name, ...rest] = argv;
  // The known-scenario list is not printed here: it would mean importing
  // `../scenarios` before the replay env is restored, and `main` names them once
  // the registry is loaded anyway.
  if (!name) throw new Error(SCENARIO_RUN_USAGE);
  const args: ScenarioRunArgs = { name, release: false };
  for (let i = 0; i < rest.length; i += 1) {
    const flag = rest[i];
    if (flag === '--release') {
      args.release = true;
      continue;
    }
    if (flag === '--trace') {
      const value = rest[i + 1];
      if (value === undefined) throw new Error('--trace needs a path');
      i += 1;
      args.trace = value;
      continue;
    }
    // What the user typed prints as its shape: stderr is pasted, and a stray
    // argument can be a path or a name (#576 W51) - unless it is shaped like a
    // flag or a number, which neither can be (#576 W58 Q4).
    if (flag !== '--seed' && flag !== '--iterations') throw new Error(`unknown flag, ${cliValueShape(flag)}`);
    const raw = rest[i + 1];
    if (raw === undefined) throw new Error(`flag ${flag} needs a value`);
    i += 1;
    if (flag === '--seed') args.seed = integerFlag(flag, raw);
    else args.iterations = positiveIntFlag(flag, raw);
  }
  return args;
}

/**
 * [#576 NF-E12-1] The seed and iteration count a run passes to `runScenario`.
 *
 * A replay runs at the trace's own: the recording's seed and iteration count are
 * part of the repro like its environment is. Without them a replay ran at the
 * scenario's defaults - a fuzz scenario whose every iteration replays the whole
 * trace (undoPersistenceFuzz) ran a 1-cycle recording 12 times, and a seed-16
 * recording reported seed 1 and the swarm that seed draws. An explicit `--seed` /
 * `--iterations` still wins, so a replay can be re-run under other numbers on
 * purpose. Without a trace, only the flags given are passed (the scenario's
 * defaults apply to the rest).
 */
export function scenarioRunOptions(
  args: Pick<ScenarioRunArgs, 'seed' | 'iterations'>,
  trace: Pick<FuzzTrace, 'seed' | 'iterations'> | null,
): { seed?: number; iterations?: number } {
  // A trace is a file from wherever the user got it: its numbers get the same
  // checks the flags do, rather than a 0-iteration "PASS".
  if (trace && args.seed === undefined && !Number.isInteger(trace.seed)) {
    throw new Error(`the trace's seed ${valueShape(trace.seed)} is not an integer; pass --seed`);
  }
  if (trace && args.iterations === undefined && !(Number.isInteger(trace.iterations) && trace.iterations >= 1)) {
    throw new Error(
      `the trace's iteration count ${valueShape(trace.iterations)} is not a positive integer; pass --iterations`,
    );
  }
  const seed = args.seed ?? trace?.seed;
  const iterations = args.iterations ?? trace?.iterations;
  return {
    ...(seed === undefined ? {} : { seed }),
    ...(iterations === undefined ? {} : { iterations }),
  };
}
