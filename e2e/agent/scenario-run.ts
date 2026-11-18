/**
 * Run ONE stress scenario against a live app session, at a chosen seed and
 * iteration count, then a full oracle sweep. The companion to
 * `scenarios-smoke.ts` (which runs every scenario once at small n): this is the
 * one to reach for when a seeded scenario needs re-running - a fuzzer repro, or
 * a long soak at a bigger n than the gate would tolerate.
 *
 * Run on the repo-pinned Node (`nvm use`), with a display or under xvfb:
 *
 *   pnpm agent:scenario undo-redo-fuzz --seed 7 --iterations 60
 *
 * `--trace <file>` REPLAYS a recorded trace instead of drawing moves - same
 * bridge, same oracles, same model, same failure payload; only the source of the
 * decisions changes. A trace is dropped into the session artifacts dir whenever
 * a fuzz run fails, so the usual loop is "run, fail, replay the trace it left,
 * then `pnpm agent:fuzz-shrink` it".
 *
 * A replay RESTORES the environment the recording ran under (fixture, tolerance,
 * stream, the persistence knobs) before the scenario modules are imported, and
 * refuses a trace recorded by a different scenario. Both matter: the recorded
 * steps carry RESOLVED line and shot ids, so replaying them against another
 * fixture drives moves at ids that do not exist there and reports an invented
 * failure - or passes green - while the operator believes they are looking at
 * the recorded bug.
 *
 * `--release` (or `STALAGMIGHT_AGENT_RELEASE=1`) drives the release binary
 * instead of the debug one. Opt-in on purpose: a debug build's `debug_assert!`s
 * and `debug_assertions`-gated self-checks are oracles in their own right, so
 * the default keeps them. See docs/agent-harness.md.
 *
 * Exits non-zero when the scenario throws or the sweep reports a fatal finding,
 * so it works as a one-off gate too.
 */
import * as path from 'path';
import { readTrace, restoreTraceEnv } from './lib/fuzzTrace';

interface Args {
  name: string;
  seed?: number;
  iterations?: number;
  release: boolean;
  /** Replay a recorded trace instead of drawing moves. */
  trace?: string;
}

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
function integerFlag(flag: string, raw: string): number {
  if (!/^-?\d+$/.test(raw)) throw new Error(`flag ${flag} needs an integer, got "${raw}"`);
  return Number(raw);
}

function positiveIntFlag(flag: string, raw: string): number {
  if (!/^\d+$/.test(raw) || Number(raw) < 1) {
    throw new Error(`flag ${flag} needs a positive integer, got "${raw}"`);
  }
  return Number(raw);
}

function parseArgs(argv: string[]): Args {
  const [name, ...rest] = argv;
  if (!name) {
    // The known-scenario list is NOT printed here: `./scenarios` is imported
    // after the replay env is restored, and importing it early to build this
    // message would freeze the fuzz scenarios' knobs against the wrong
    // environment. `main` names them once the registry is loaded.
    throw new Error('usage: pnpm agent:scenario <name> [--seed N] [--iterations M] [--release] [--trace FILE]');
  }
  const args: Args = { name, release: false };
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
    if (flag !== '--seed' && flag !== '--iterations') throw new Error(`unknown flag ${flag}`);
    const raw = rest[i + 1];
    if (raw === undefined) throw new Error(`flag ${flag} needs a value`);
    i += 1;
    if (flag === '--seed') args.seed = integerFlag(flag, raw);
    else args.iterations = positiveIntFlag(flag, raw);
  }
  return args;
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));

  // Set BEFORE `session.start()`: `agentBuildMode()` is read at call time, which
  // is what lets a flag parsed after this module's imports still take effect.
  if (args.release) process.env.STALAGMIGHT_AGENT_RELEASE = '1';
  // Same reason, and it is how replay reaches the scenario: the scenarios read
  // their knobs from the environment, so the flag is just a nicer spelling of
  // the env var and the two are interchangeable.
  if (args.trace) {
    const tracePath = path.resolve(args.trace);
    const trace = readTrace(tracePath);
    if (trace.scenario !== args.name) {
      throw new Error(
        `trace ${tracePath} was recorded by "${trace.scenario}", not "${args.name}"; ` +
          `its steps carry that scenario's resolved ids. Run it as: ` +
          `pnpm agent:scenario ${trace.scenario} --trace ${args.trace}`,
      );
    }
    // The recording's own environment is part of the repro. Restored here, i.e.
    // before `./scenarios` is imported below, because the fuzz scenarios freeze
    // FIXTURE / ALLOW_KNOWN_BUGS / REPAIR / DUMP_HISTORY into module-level
    // consts at import time; a restore after that would be a no-op.
    //
    // Through `restoreTraceEnv`, which applies the recorder's own prefix filter:
    // a trace is a file from wherever the user got it, and this process goes on
    // to invoke cargo and launch the app.
    const changed = new Set(
      Object.entries(trace.env)
        .filter(([key, value]) => process.env[key] !== value)
        .map(([key]) => key),
    );
    const { applied, refused, cleared } = restoreTraceEnv(trace.env);
    process.env.STALAGMIGHT_FUZZ_REPLAY = tracePath;
    const restored = applied.filter(([key]) => changed.has(key));
    process.stdout.write(
      `replaying ${trace.steps.length} step(s) recorded by ${trace.scenario} on ${trace.fixture} ` +
        `(seed ${trace.seed}, rev ${trace.gitRev.slice(0, 12)})\n` +
        (restored.length > 0
          ? `  restored from the trace: ${restored.map(([key, value]) => `${key}=${value}`).join(' ')}\n`
          : '') +
        // Printed for the same reason `restored` is: a knob the shell set and
        // the recording did not is a difference between this run and that one,
        // and silently dropping it would be a replay that quietly is not the
        // recording. The recording ran on those knobs' DEFAULTS.
        (cleared.length > 0
          ? `  cleared ${cleared.length} knob(s) the recording did not have, back to their defaults: ` +
            `${cleared.join(', ')}\n`
          : '') +
        (refused.length > 0
          ? `  REFUSED ${refused.length} env key(s) the recorder would never write: ${refused.join(', ')}\n`
          : ''),
    );
  }

  // Imported after the replay env is in place - see the comment above.
  const { getScenario, listScenarios, runScenario } = await import('./scenarios');
  // Through the platform factory, like every other runner. Naming `AppSession`
  // here pinned this runner to the Linux backend, so on macOS every
  // `agent:scenario` died as the 15 s "tauri-driver did not start listening"
  // timeout that `createSession`'s own comment says was replaced by a named
  // refusal in #422 - and the macOS backend it should pick (`MacSession`,
  // tauri-wd) was unreachable from here.
  const { createSession } = await import('./createSession');
  const { runSweep, newOracleState } = await import('./lib/oracles');
  if (!getScenario(args.name)) {
    throw new Error(`unknown scenario "${args.name}"; known: ${listScenarios().join(', ')}`);
  }

  const session = createSession();
  const started = await session.start({ build: 'auto' });
  process.stdout.write(
    `session ${started.state} (${started.buildMode} build ${started.buildAction})\n`,
  );
  let failed = false;

  try {
    const metrics = await runScenario(args.name, session, {
      ...(args.seed === undefined ? {} : { seed: args.seed }),
      ...(args.iterations === undefined ? {} : { iterations: args.iterations }),
      log: (line) => process.stdout.write(`   . ${line}\n`),
    });
    const sweep = await runSweep(session, newOracleState(), { full: true, recorder: session.recorder });
    const fatal = sweep.findings.filter((finding) => finding.severity === 'fatal');
    if (fatal.length) failed = true;
    process.stdout.write(
      `${fatal.length ? 'FAIL' : 'PASS'}: ${args.name} seed=${metrics.seed} iterations=${metrics.iterations} ` +
        `mode=${started.buildMode} ` +
        `(${metrics.durationMs}ms) ${JSON.stringify(metrics.metrics)}` +
        `${sweep.findings.length ? ` findings=${JSON.stringify(sweep.findings)}` : ''}\n`,
    );
  } catch (err) {
    failed = true;
    process.stdout.write(`FAIL: ${args.name} threw:\n${err instanceof Error ? err.message : String(err)}\n`);
  } finally {
    await session.stop('scenario-run');
  }

  process.exit(failed ? 1 : 0);
}

main().catch((err) => {
  process.stderr.write(`fatal: ${err instanceof Error ? err.stack : String(err)}\n`);
  process.exit(1);
});
