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
 * stream, the persistence knobs) before the scenario modules are imported, runs
 * at the recording's seed and iteration count unless `--seed` / `--iterations`
 * say otherwise, and refuses a trace recorded by a different scenario. Both matter: the recorded
 * steps carry RESOLVED section and shot ids, so replaying them against another
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
import { readTrace, restoreTraceEnv, type FuzzTrace } from './lib/fuzzTrace';
import { parseScenarioArgs, scenarioRunOptions } from './lib/scenarioRunArgs';
import {
  cliValueShape,
  foldLocalPaths,
  pairFailOutput,
  progressLogger,
  scenarioFailOutput,
  shareableErrorText,
  valueShape,
} from './lib/shareableFailure';
import { shareableFixtureName } from './lib/privateFixtures';
import { shareableKnobValue } from './lib/traceKnobs';

/**
 * Values from outside as printed before the registry is loaded (#576 W51) - the
 * CLI, and a trace from wherever the user got it: a scenario name of the
 * registry's shape (lower-case words, digits and dashes, as `fuzz-matrix
 * --scenario` validates it), a knob name of the harness's (`STALAGMIGHT_` and
 * upper-case words) and a revision print as they are, anything else by its
 * shape (`valueShape`: a number as itself).
 */
const scenarioName = (name: string): string => (/^[a-z][a-z0-9-]{1,63}$/.test(name) ? name : valueShape(name));
const envKey = (key: string): string => (/^STALAGMIGHT_[A-Z0-9_]{1,64}$/.test(key) ? key : valueShape(key));
const gitRev = (rev: unknown): string =>
  typeof rev === 'string' && /^(?:[0-9a-f]{7,40}|unknown)$/.test(rev) ? rev.slice(0, 12) : valueShape(rev);

async function main(): Promise<void> {
  const args = parseScenarioArgs(process.argv.slice(2));
  let trace: FuzzTrace | null = null;

  // Set BEFORE `session.start()`: `agentBuildMode()` is read at call time, which
  // is what lets a flag parsed after this module's imports still take effect.
  if (args.release) process.env.STALAGMIGHT_AGENT_RELEASE = '1';
  // Same reason, and it is how replay reaches the scenario: the scenarios read
  // their knobs from the environment, so the flag is just a nicer spelling of
  // the env var and the two are interchangeable.
  if (args.trace) {
    const tracePath = path.resolve(args.trace);
    trace = readTrace(tracePath);
    if (trace.scenario !== args.name) {
      throw new Error(
        `trace ${foldLocalPaths(tracePath)} was recorded by "${scenarioName(trace.scenario)}", not "${scenarioName(args.name)}"; ` +
          `its steps carry that scenario's resolved ids. Run it as: ` +
          `pnpm agent:scenario ${scenarioName(trace.scenario)} --trace ${foldLocalPaths(args.trace)}`,
      );
    }
    // The recording's own environment is part of the repro. Every knob is read
    // at call time (`lib/fuzzKnobs.ts`, undoRedoFuzz's `repairEnabled`), so any
    // point before the run would do; it is restored here, before `./scenarios`
    // is even imported, so that stays true should a module ever read one at
    // import time. Its seed and iteration count are the other half of the repro
    // and go to `runScenario` below (`scenarioRunOptions`, #576 NF-E12-1).
    //
    // Through `restoreTraceEnv`, which applies the recorder's own prefix filter:
    // a trace is a file from wherever the user got it, and this process goes on
    // to invoke cargo and launch the app.
    const changed = new Set(
      Object.entries(trace.env)
        .filter(([key, value]) => process.env[key] !== value)
        .map(([key]) => key),
    );
    const { applied, refused, cleared, withheld } = restoreTraceEnv(trace.env, trace.envWithheld);
    process.env.STALAGMIGHT_FUZZ_REPLAY = tracePath;
    const restored = applied.filter(([key]) => changed.has(key));
    const runAt = scenarioRunOptions(args, trace);
    process.stdout.write(
      `replaying ${trace.steps.length} step(s) recorded by ${scenarioName(trace.scenario)} on ${shareableFixtureName(trace.fixture)} ` +
        `(seed ${valueShape(trace.seed)}, ${valueShape(trace.iterations)} iteration(s), rev ${gitRev(trace.gitRev)}); ` +
        `running at seed ${String(runAt.seed)}, ${String(runAt.iterations)} iteration(s)\n` +
        (restored.length > 0
          ? `  restored from the trace: ${restored.map(([key, value]) => `${envKey(key)}=${shareableKnobValue(key, value)}`).join(' ')}\n`
          : '') +
        // Printed for the same reason `restored` is: a knob the shell set and
        // the recording did not is a difference between this run and that one,
        // and silently dropping it would be a replay that quietly is not the
        // recording. The recording ran on those knobs' DEFAULTS.
        (cleared.length > 0
          ? `  cleared ${cleared.length} knob(s) the recording did not have, back to their defaults: ` +
            `${cleared.map(envKey).join(', ')}\n`
          : '') +
        (refused.length > 0
          ? `  REFUSED ${refused.length} env key(s) the recorder would never write: ${refused.map(envKey).join(', ')}\n`
          : '') +
        // The recording had these set to a value its trace does not carry (not
        // one its knob takes, #576 W58 Q9): this shell's value stands.
        (withheld.length > 0
          ? `  the recording set ${withheld.length} knob(s) to a value the trace does not carry; ` +
            `left as this shell has them: ${withheld.map(envKey).join(', ')}\n`
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
  const { findingsSummary, runSweep, newOracleState } = await import('./lib/oracles');
  const scenario = getScenario(args.name);
  if (!scenario) {
    // Not a registry name, so its shape (#576 W51): it is the user's text.
    throw new Error(`unknown scenario, ${cliValueShape(args.name)}; known: ${listScenarios().join(', ')}`);
  }

  // Determinism mode (`lib/fuzzDeterminismRun.ts`). `=1` pairs inside
  // `runScenario`; `=2sessions` needs a session per replay, which only this
  // runner owns.
  const { DETERMINISM_SCENARIOS } = await import('./scenarios');
  const { fuzzDeterminism, fuzzDeterminismPlant, fuzzFixture } = await import('./lib/fuzzKnobs');
  const determinism = fuzzDeterminism();
  if (determinism !== 'off' && !DETERMINISM_SCENARIOS.has(args.name)) {
    throw new Error(
      `STALAGMIGHT_FUZZ_DETERMINISM covers ${[...DETERMINISM_SCENARIOS].join(', ')}; "${args.name}" has no probe hooks`,
    );
  }
  if (determinism === 'two-sessions') {
    const { runDeterminismPair } = await import('./lib/fuzzDeterminismRun');
    const runAt = scenarioRunOptions(args, trace);
    // Shareable progress lines (#576 W23-NF3), as on the one-session path; a
    // replay's own lines also go to its session's actions.jsonl.
    const print = (line: string): void => {
      process.stdout.write(`   . ${line}\n`);
    };
    const log = progressLogger(print);
    // Each replay's action log, for the pair's own verdict once both stopped.
    const replayLogs: { event(event: string, detail?: unknown): void }[] = [];
    let pairFailed = false;
    try {
      const metrics = await runDeterminismPair(
        {
          scenario: args.name,
          // Numbers whatever the trace held (#576 W51), as `runScenario` reads them.
          seed: Number(runAt.seed ?? 1),
          iterations: Number(runAt.iterations ?? scenario.defaultIterations),
          fixture: fuzzFixture(),
          mode: 'two-sessions',
          plantAt: fuzzDeterminismPlant(),
          log,
        },
        async (replay) => {
          const own = createSession();
          const ownStart = await own.start({ build: 'auto' });
          if (own.hasRecorder) replayLogs.push(own.recorder);
          log(`replay ${replay}: session ${ownStart.state} (${ownStart.buildMode} build ${ownStart.buildAction})`);
          try {
            const replayMetrics = await runScenario(args.name, own, {
              ...runAt,
              log: progressLogger(print, own.hasRecorder ? own.recorder : undefined),
            });
            const sweep = await runSweep(own, newOracleState(), { full: true, recorder: own.recorder });
            const fatal = sweep.findings.filter((finding) => finding.severity === 'fatal');
            if (fatal.length > 0) {
              // Counts per severity, kind and code only (#576 HFI2-F2); the
              // findings' text is in this replay's actions.jsonl.
              throw new Error(`replay ${replay}: fatal findings=${findingsSummary(fatal)}`);
            }
            return replayMetrics;
          } catch (err) {
            // The full text goes to this replay's actions.jsonl (#576 W21-NF1).
            if (own.hasRecorder) {
              own.recorder.event('scenario-failure', {
                scenario: args.name,
                replay,
                message: err instanceof Error ? err.message : String(err),
              });
            }
            throw err;
          } finally {
            await own.stop('scenario-run');
          }
        },
      );
      process.stdout.write(
        `PASS: ${args.name} seed=${metrics.seed} iterations=${metrics.iterations} determinism=2sessions ` +
          `(${metrics.durationMs}ms) ${JSON.stringify(metrics.metrics)}\n`,
      );
    } catch (err) {
      pairFailed = true;
      // The shareable form, as on the one-session path (#576 W21-NF1); the full
      // text goes to each replay's actions.jsonl, as both sessions have stopped.
      process.stdout.write(pairFailOutput(args.name, err, replayLogs));
    }
    process.exit(pairFailed ? 1 : 0);
  }

  const session = createSession();
  const started = await session.start({ build: 'auto' });
  process.stdout.write(
    `session ${started.state} (${started.buildMode} build ${started.buildAction})\n`,
  );
  let failed = false;

  try {
    const metrics = await runScenario(args.name, session, {
      // A replay runs at the trace's seed and iteration count unless a flag says
      // otherwise (#576 NF-E12-1); a fresh run passes only the flags given.
      ...scenarioRunOptions(args, trace),
      // Shareable progress lines (#576 W23-NF3); the full text goes to actions.jsonl.
      log: progressLogger(
        (line) => process.stdout.write(`   . ${line}\n`),
        session.hasRecorder ? session.recorder : undefined,
      ),
    });
    const sweep = await runSweep(session, newOracleState(), { full: true, recorder: session.recorder });
    const fatal = sweep.findings.filter((finding) => finding.severity === 'fatal');
    if (fatal.length) failed = true;
    process.stdout.write(
      `${fatal.length ? 'FAIL' : 'PASS'}: ${args.name} seed=${metrics.seed} iterations=${metrics.iterations} ` +
        `mode=${started.buildMode} ` +
        `(${metrics.durationMs}ms) ${JSON.stringify(metrics.metrics)}` +
        // Counts per severity, kind and code only (#576 HFI2-F2): a finding's
        // message is the app's log line, which names sections and projects of
        // the loaded data. The text stays in the session's actions.jsonl.
        `${sweep.findings.length ? ` findings=${findingsSummary(sweep.findings)}` : ''}\n`,
    );
  } catch (err) {
    failed = true;
    // The shareable form - headline, reproduce/trace lines, diff paths and
    // counts - is what this prints (#576 W21-NF1): the full message quotes the
    // app (integrity messages, IPC rejection lines, diff values). The full text
    // goes to the session's actions.jsonl, which is not for sharing.
    if (session.hasRecorder) {
      session.recorder.event('scenario-failure', {
        scenario: args.name,
        message: err instanceof Error ? err.message : String(err),
      });
    }
    process.stdout.write(scenarioFailOutput(args.name, err));
  } finally {
    await session.stop('scenario-run');
  }

  process.exit(failed ? 1 : 0);
}

main().catch((err) => {
  // The shareable crash form (#576 W23-NF2): a crash in session setup can quote
  // the app (an IPC rejection, a page script's error).
  process.stderr.write(`fatal: ${shareableErrorText(err)}\n`);
  process.exit(1);
});
