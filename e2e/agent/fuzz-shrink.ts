/**
 * Automatic minimization of a recorded fuzz trace.
 *
 *   pnpm agent:fuzz-shrink <trace.json> [--release] [--max-tests N] [--out FILE]
 *
 * Give it the trace a failing fuzz run left in the session artifacts dir and it
 * returns the SHORTEST subsequence of that run's moves which still fails the same
 * way, plus a numbered action list you can transcribe straight into a Rust test.
 * That is the step triage used to do by hand.
 *
 * ## How a replay is run
 *
 * ONE app session for the whole shrink, and one `runScenario` call per candidate.
 * A scenario already resets everything it depends on at the start of a run - it
 * closes whatever project is open, materializes a FRESH copy of the fixture,
 * deletes that copy's persisted undo history, re-injects the oracle (which resets
 * the snapshot model) and asserts both stacks came up empty - so a replay
 * inherits nothing from the one before it. Paying a process start per candidate
 * instead would roughly quadruple the wall clock for no isolation the scenario
 * does not already provide, and the assertion that both stacks are empty is the
 * tripwire if that ever stops being true.
 *
 * ## What "still fails the same way" means
 *
 * The target signature is taken from the FIRST replay of the unshrunk trace, not
 * from the header the recording wrote. Those should agree, and the run says so
 * when they do not - but if the build has moved since the recording, the honest
 * baseline is what this binary actually does now, and a shrink against a stale
 * signature would reject everything.
 *
 * A candidate that fails DIFFERENTLY is rejected exactly like one that passes
 * (`fuzzShrink.ts` documents why, and the divergent count is reported), so a
 * shrink cannot wander off onto one of the other ten open bugs.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

import {
  describeSignature,
  describeSteps,
  failureSignature,
  readTrace,
  restoreTraceEnv,
  sameSignature,
  shareableSignature,
  shrinkableSteps,
  writeTrace,
  type FailureSignature,
  type FuzzTrace,
  type TraceStep,
} from './lib/fuzzTrace';
import { ddmin, type ShrinkVerdict } from './lib/fuzzShrink';
import { shareableErrorText } from './lib/shareableFailure';
// Safe to import statically, unlike the scenarios below: this module freezes
// nothing at import and reads `STALAGMIGHT_FUZZ_CORPUS_DIR` at call time, which
// its own tests pin.
import { CORPUS_DIR_ENV, corpusAvailable, corpusLookup, redactCorpusText } from './lib/fixtureCorpus';

interface Args {
  trace: string;
  release: boolean;
  maxTests: number;
  out: string | null;
}

function parseArgs(argv: string[]): Args {
  const [trace, ...rest] = argv;
  if (!trace) {
    throw new Error('usage: pnpm agent:fuzz-shrink <trace.json> [--release] [--max-tests N] [--out FILE]');
  }
  const args: Args = { trace: path.resolve(trace), release: false, maxTests: 0, out: null };
  for (let i = 0; i < rest.length; i += 1) {
    const flag = rest[i];
    if (flag === '--release') {
      args.release = true;
      continue;
    }
    const value = rest[i + 1];
    if (value === undefined) throw new Error(`flag ${flag} needs a value`);
    i += 1;
    if (flag === '--max-tests') args.maxTests = Number(value);
    else if (flag === '--out') args.out = path.resolve(value);
    else throw new Error(`unknown flag ${flag}`);
  }
  return args;
}

/** Outcome of replaying one candidate: null when it passed. */
type ReplayOutcome = { message: string; signature: FailureSignature } | null;

/**
 * Exit code, returned rather than taken here: an early `return` from inside the
 * `try` used to skip a trailing `process.exit`, so the "replayed green" path -
 * the one that means the trace does NOT reproduce - exited 0 and read as
 * success to anything using this as a regression check. Every path now carries
 * its own code and the caller is the only place that exits.
 */
async function main(): Promise<number> {
  const args = parseArgs(process.argv.slice(2));
  const trace = readTrace(args.trace);
  if (args.release) process.env.STALAGMIGHT_AGENT_RELEASE = '1';

  // The env the recording ran under is part of the repro: tolerance, fixture,
  // stream and the persistence knobs all change what a replay does. Restore it
  // rather than inheriting whatever shell the shrink was started from - through
  // the recorder's own prefix filter, because a trace is a file from wherever
  // the user got it and this process goes on to invoke cargo and launch the app.
  const { refused, cleared } = restoreTraceEnv(trace.env);
  if (refused.length > 0) {
    process.stdout.write(
      `REFUSED ${refused.length} env key(s) the recorder would never write: ${refused.join(', ')}\n`,
    );
  }
  if (cleared.length > 0) {
    // A knob this shell set and the recording did not. Unset here so the
    // recording's DEFAULT applies - said out loud, because it is a difference
    // between the two runs and every replay in the shrink inherits it.
    process.stdout.write(
      `CLEARED ${cleared.length} knob(s) the recording did not have, back to their defaults: ` +
        `${cleared.join(', ')}\n`,
    );
  }

  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-shrink-'));
  const candidatePath = path.join(scratch, 'candidate.json');
  // Point the scenario's own trace-on-failure at the scratch dir, so a shrink
  // does not litter the artifacts dir with one file per rejected candidate.
  process.env.STALAGMIGHT_FUZZ_TRACE = path.join(scratch, 'replay-out.json');
  process.env.STALAGMIGHT_FUZZ_REPLAY = candidatePath;

  // IMPORTED HERE, not at the top of the file, and that is the whole point of
  // the restore above: `undoRedoFuzz` / `undoPersistenceFuzz` freeze FIXTURE,
  // ALLOW_KNOWN_BUGS, REPAIR and DUMP_HISTORY into module-level consts when
  // they are first imported. A static import would evaluate them against the
  // invoking shell's environment, and the restore would then be a no-op -
  // replaying a naharon trace against the default fixture, or replaying a
  // tolerant recording in STRICT mode, and shrinking toward the wrong defect.
  const { getScenario, runScenario } = await import('./scenarios');
  const { AppSession } = await import('./session');
  if (!getScenario(trace.scenario)) {
    throw new Error(`trace names scenario "${trace.scenario}", which this build does not have`);
  }

  const session = new AppSession();
  const started = await session.start({ build: 'auto' });
  process.stdout.write(
    `session ${started.state} (${started.buildMode} build ${started.buildAction})\n` +
      `trace ${args.trace}\n` +
      `  scenario=${trace.scenario} seed=${trace.seed} fixture=${trace.fixture} ` +
      `steps=${trace.steps.length} rev=${trace.gitRev.slice(0, 12)}\n` +
      `  env=${JSON.stringify(trace.env)}\n`,
  );

  // Said BEFORE the baseline replay, not after it, and it RESOLVES the ids
  // rather than just checking the knob.
  //
  // A trace whose corpus draws all skip replays green, and the green-baseline
  // message reads as "the bug is fixed" - so without this an operator waits out
  // a full replay and then draws the wrong conclusion from it. Measured twice
  // in a row on the F3 trace this shrinker was validated against: once with the
  // knob unset, and once with it set to a DIFFERENT corpus, where every id
  // resolved to nothing and the replay drifted from step one. The second is why
  // "is the knob set" is not the question - only resolving the ids is.
  //
  // A corpus root is never named in the output, here or anywhere else; the
  // count and the ids are.
  const corpusIds = trace.steps.flatMap((step) => (step.kind === 'corpusFile' ? [step.corpusHash ?? step.corpusId] : []));
  const unresolved = corpusIds.filter((reference) => corpusLookup(reference).outcome !== 'found');
  if (unresolved.length > 0) {
    process.stdout.write(
      `WARNING: ${unresolved.length} of ${corpusIds.length} CORPUS draw(s) do not resolve here` +
        `${corpusAvailable() ? ' - the configured corpus is not the one this was recorded against' : ` - ${CORPUS_DIR_ENV} is not set`}.\n` +
        '  Those steps will be skipped and the replay will very likely not reproduce.\n' +
        '  A trace never carries that path (it would let a file decide which tree this process walks\n' +
        `  and copies out of), so ${CORPUS_DIR_ENV} has to come from the shell that starts the shrink.\n`,
    );
  }

  const replayTimesMs: number[] = [];
  const replay = async (steps: readonly TraceStep[]): Promise<ReplayOutcome> => {
    writeTrace(candidatePath, { ...trace, steps: [...steps], failure: null });
    const started = Date.now();
    try {
      await runScenario(trace.scenario, session, { seed: trace.seed, iterations: trace.iterations });
      return null;
    } catch (err) {
      // Redacted before it is printed, compared or written into the minimal
      // trace: a replay that copied a corpus entry can fail on a message that
      // quotes it (#576 HFI-13).
      const message = redactCorpusText(err instanceof Error ? err.message : String(err));
      return { message, signature: failureSignature(message) };
    } finally {
      replayTimesMs.push(Date.now() - started);
    }
  };

  const wallStart = Date.now();
  let failed = false;
  let minimal: TraceStep[] = trace.steps;

  try {
    // 1. Establish the baseline by replaying the whole trace.
    const baseline = await replay(trace.steps);
    if (!baseline) {
      process.stdout.write(
        'the trace REPLAYED GREEN: nothing to shrink.\n' +
          '  Either the bug is fixed, or the failure needed something the trace does not carry.\n' +
          (unresolved.length > 0
            ? `  START HERE: ${unresolved.length} corpus draw(s) did not resolve - see the warning above.\n`
            : ''),
      );
      failed = true;
      return 1;
    }
    // What is printed and compared with the trace header is the SHAREABLE
    // signature (#576 W21-NF1): the full one can quote the app's text, and a
    // written trace carries only the shareable one. ddmin below still compares
    // full signatures, in memory. The full baseline goes to the session's
    // actions.jsonl, which is not for sharing.
    if (session.hasRecorder) session.recorder.event('scenario-failure', { scenario: trace.scenario, message: baseline.message });
    const published = shareableSignature(baseline.message);
    process.stdout.write(`baseline failure reproduced in ${replayTimesMs[0]} ms:\n${describeSignature(published)}\n`);
    if (trace.failure && !sameSignature(shareableSignature(trace.failure.message), published)) {
      process.stdout.write(
        'NOTE: the replay signature differs from the one recorded in the trace header.\n' +
          `  recorded:\n${describeSignature(shareableSignature(trace.failure.message))}\n` +
          '  Shrinking against the REPLAYED signature, which is what this build does now.\n',
      );
    }
    const target = baseline.signature;

    // 2. ddmin - over the MOVES, with the structure held out of its reach.
    //
    // `episode` and `flow` steps are boundaries, not moves. Given to ddmin as
    // ordinary elements it deletes them like anything else, and deleting the
    // marker that opens a flow does not remove a step: it MERGES that flow into
    // the one before it. `planReplay` then measures one delta across two
    // imports, F3's headline normalizes digits so the merged failure signs
    // identically to the recorded one, and the divergence check - the thing
    // that is supposed to stop a shrink wandering onto another bug - waves it
    // through. The output would be a minimized trace failing on a boundary no
    // run ever had.
    //
    // So the candidates are move indices and `rebuild` restores the recorded
    // grouping around whatever survived. See `shrinkableSteps`.
    const structure = shrinkableSteps(trace.steps);
    const result = await ddmin(structure.payload, {
      maxTests: args.maxTests,
      test: async (candidate): Promise<ShrinkVerdict> => {
        const outcome = await replay(structure.rebuild(candidate));
        if (!outcome) return 'passed';
        return sameSignature(outcome.signature, target) ? 'preserved' : 'divergent';
      },
      onProgress: (info) => {
        process.stdout.write(
          `  [${String(info.stats.tests).padStart(3)}] tried ${String(info.candidateSize).padStart(3)} move(s) ` +
            `-> ${info.verdict} (best ${info.bestSize})\n`,
        );
      },
    });
    minimal = structure.rebuild(result.minimal);

    const wallMs = Date.now() - wallStart;
    const perReplay = replayTimesMs.reduce((sum, ms) => sum + ms, 0) / Math.max(replayTimesMs.length, 1);
    const outPath = args.out ?? args.trace.replace(/\.json$/, '') + '.min.json';
    writeTrace(outPath, {
      ...trace,
      steps: minimal,
      failure: { message: baseline.message, signature: target },
    });

    process.stdout.write(
      [
        '',
        `SHRUNK ${trace.steps.length} -> ${minimal.length} step(s), ` +
          `${structure.payload.length} -> ${result.minimal.length} move(s)` +
          `${args.maxTests > 0 && result.stats.tests >= args.maxTests ? ' (budget exhausted: SMALLEST FOUND, not proven 1-minimal)' : ' (1-minimal: no single move can still be removed)'}`,
        `  replays: ${result.stats.tests} (+${result.stats.cacheHits} cached) - ` +
          `${result.stats.preserved} preserved, ${result.stats.passed} lost the failure, ` +
          `${result.stats.divergent} failed DIFFERENTLY and were rejected`,
        `  wall clock: ${(wallMs / 1000).toFixed(1)} s, ${Math.round(perReplay)} ms per replay ` +
          `(${replayTimesMs.length} replays incl. the baseline)`,
        `  minimal trace: ${outPath}`,
        '',
        'minimal action list:',
        describeSteps(minimal),
        '',
      ].join('\n'),
    );
  } catch (err) {
    failed = true;
    // Shareable crash form (#576 W23-NF2), like every other line this prints.
    process.stdout.write(`shrink failed: ${shareableErrorText(err)}\n`);
  } finally {
    await session.stop('fuzz-shrink');
    fs.rmSync(scratch, { recursive: true, force: true });
  }

  return failed ? 1 : 0;
}

main().then(
  (code) => process.exit(code),
  (err: unknown) => {
    process.stderr.write(`fatal: ${shareableErrorText(err)}\n`);
    process.exit(1);
  },
);

/** Re-exported for the type checker's benefit; the CLI is the only consumer. */
export type { FuzzTrace };
