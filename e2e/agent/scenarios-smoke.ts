/**
 * Runs every stress scenario once at small iteration counts against a live app
 * session, printing pass/fail + metrics. This is the Phase 5 scenario gate:
 * proves each scenario drives the real app without throwing or tripping a fatal
 * oracle finding. Run on the repo-pinned Node (`nvm use`):
 *
 *   pnpm agent:scenarios
 */
import { createSession } from './createSession';
import { listScenarios, runScenario } from './scenarios';
import { findingsSummary, runSweep, newOracleState } from './lib/oracles';
import { progressLogger, scenarioFailOutput, shareableErrorText } from './lib/shareableFailure';
import { callStoreAction } from './lib/stores';

// Small iteration counts for a fast gate; the real stress values are the
// scenarios' defaultIterations.
const SMOKE_ITERATIONS: Record<string, number> = {
  'seed-large-project': 1,
  'undo-redo-churn': 5,
  // GREEN, and STRICT. This count is not arbitrary: seed 1 x40 on the v2
  // generator stream is the repro line for issue #413 bug 1 (a redo of
  // CreateSection re-minting every shot uuid, plus the gauge-anchor half), so
  // this entry is that bug's standing regression check. It is also finding 11's
  // residue's: after bug 1 was fixed this seed went on failing, and
  // `pnpm agent:fuzz-shrink` cut it to two steps - executeBatch(...+crtSec+...)
  // then save - which is what identified and then closed that residue
  // (b9d6b14b). Both reductions are written up in the scenario header.
  //
  // Every classifier and every known-bug tolerance is deleted, so
  // STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1 changes nothing: a mismatch of any shape
  // fails. Do not lower the count - a shorter run stops reaching the sequences
  // both bugs were minimized to - and if this ever goes red again, do not add a
  // classifier back: the red would be the app's.
  'undo-redo-fuzz': 40,
  // GREEN, and strict, for the same reasons. ONE iteration is what the entry is
  // set to - the scenario is not a random walk, so every iteration builds the
  // same 19-entry covering stack and crosses the same save/reload boundary; a
  // second iteration adds ~5 s and no new signal.
  //
  // What that one iteration used to hit, deterministically: finding 12 (undo of
  // set_shot_kind / update_shot editing the wrong shot after a reload) on the
  // first id-bearing entry the unwind reached, which is where a strict run
  // stopped; tolerantly it went on to report bug 1, bug 8 and finding 11 as
  // well. All four are fixed, so the same iteration now walks the whole restored
  // stack down and back up clean (verified on fractal seeds 1, 3 and 9, and with
  // STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET=1) - which is exactly what makes it
  // a regression check. Raise it only if someone wants a longer soak here rather
  // than in `pnpm agent:scenario`.
  //
  // Worth running on naharon by hand now and then, since the gate only ever sees
  // the default fixture: finding 27 (an undo dropping an exactly-zero
  // section_offsets key) was found at STALAGMIGHT_FUZZ_FIXTURE=naharon.stalx
  // --seed 5 and nowhere else. Fixed in b9d6b14b / f16c96a8, and that seed is
  // clean; the write-up is in the scenario header.
  'undo-persistence-fuzz': 1,
  'dialog-cycling': 8,
  'tool-switch-spam': 20,
  'map-storm': 30,
  'save-under-churn': 4,
  'import-repeatedly': 5,
  'rapid-project-cycle': 4,
};

/**
 * Scenarios this gate does NOT run, per an explicit user decision: they execute
 * on REQUEST only - `pnpm agent:scenario`, `pnpm agent:fuzz-matrix`, or the MCP
 * `run_scenario` tool - and never in a gate.
 *
 * `import-dialog-fuzz` generates files, drives native file dialogs and walks a
 * multi-screen wizard, so one iteration costs far more wall clock than anything
 * else here and its failures want a shrink rather than a red gate.
 *
 * `export-roundtrip-fuzz` is excluded for the same reasons and two more of its
 * own: an episode rasterizes the whole survey up to three times, and its
 * round-trip oracles CREATE PROJECTS on disk and import into them. Neither
 * belongs in a gate that runs every scenario back to back on one session.
 *
 * Excluding either is NOT an expected-failure list in disguise: both are
 * expected GREEN, neither has a classifier or a tolerance, and a red from
 * either is the app's.
 */
const SMOKE_EXCLUDED = new Set(['import-dialog-fuzz', 'export-roundtrip-fuzz', 'fractal-stress']);

// There is no expected-failure list, and adding one back would be a regression.
// `undo-redo-fuzz` and `undo-persistence-fuzz` are expected GREEN and STRICT:
// every issue-413 finding is fixed in the Rust tier and every classifier and
// known-bug tolerance was deleted with it, so nothing in either scenario is
// walked past and `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1` changes nothing. If
// either one goes red, the red is the APP's - shrink it (`pnpm agent:fuzz-shrink`)
// and fix the app. Do not re-introduce an EXPECTED_RED map, a classifier, or a
// tolerance to make this gate green again.

async function main(): Promise<void> {
  const session = createSession();
  let failed = false;
  const started = await session.start({ build: 'auto' });
  process.stdout.write(`session ${started.state} (build ${started.buildAction})\n`);
  const oracle = newOracleState();

  try {
    for (const name of listScenarios()) {
      if (SMOKE_EXCLUDED.has(name)) {
        process.stdout.write(`SKIP: ${name} (runs on explicit request only)\n`);
        continue;
      }
      // Reset to a clean slate between scenarios.
      await session.exclusive(() => callStoreAction(session.browser, 'project', 'closeProject', [])).catch(() => undefined);
      try {
        const metrics = await runScenario(name, session, {
          iterations: SMOKE_ITERATIONS[name],
          // Shareable progress lines (#576 W23-NF3); the full text goes to actions.jsonl.
          log: progressLogger(
            (m) => process.stdout.write(`   · ${m}\n`),
            session.hasRecorder ? session.recorder : undefined,
          ),
        });
        const sweep = await runSweep(session, oracle, { full: true, recorder: session.recorder });
        const fatal = sweep.findings.filter((f) => f.severity === 'fatal');
        // Gate: a scenario is "green" if it did not throw and produced no FATAL
        // finding. error/warning findings are app signals the harness is meant to
        // surface (that is the point), so they are reported but do not fail the
        // scenario gate.
        if (fatal.length) failed = true;
        const label = fatal.length ? 'FAIL' : sweep.findings.length ? 'PASS+findings' : 'PASS';
        process.stdout.write(
          `${label}: ${name} (${metrics.durationMs}ms) ${JSON.stringify(metrics.metrics)}` +
            // Counts only (#576 HFI2-F2): a log finding's message names the data.
            `${sweep.findings.length ? ` findings=${findingsSummary(sweep.findings)}` : ''}\n`,
        );
      } catch (err) {
        failed = true;
        // Shareable form only (#576 W21-NF1); the full text goes to actions.jsonl.
        if (session.hasRecorder) {
          session.recorder.event('scenario-failure', { scenario: name, message: err instanceof Error ? err.message : String(err) });
        }
        process.stdout.write(scenarioFailOutput(name, err));
      }
    }
  } finally {
    await session.stop('scenarios-smoke');
  }

  process.stdout.write(failed ? '\nSCENARIOS FAILED\n' : '\nSCENARIOS PASSED\n');
  process.exit(failed ? 1 : 0);
}

main().catch((err) => {
  // The shareable crash form (#576 W23-NF2): a crash in session setup can quote
  // the app (an IPC rejection, a page script's error).
  process.stderr.write(`fatal: ${shareableErrorText(err)}\n`);
  process.exit(1);
});
