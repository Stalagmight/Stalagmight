/**
 * The environment knobs the fuzz scenarios share, each read the same way
 * wherever it is read: trace replay/record and the fixture (all four fuzz
 * scenarios), and the two diagnostic switches (the two undo fuzzers only).
 *
 * They used to be re-declared in `undoRedoFuzz.ts`, `undoPersistenceFuzz.ts`,
 * `exportRoundtripFuzz.ts` and `importDialogFuzz.ts`, each copy pointing at
 * another as the place the rule was written down. The names, the defaults and
 * the read-at-call-time rule are unchanged; only the home moved.
 *
 * EVERY knob here is read at CALL time, never frozen into a module-level const.
 * Two runners set these variables from inside their own `main()`, long after the
 * scenario modules were imported: `pnpm agent:scenario --trace` parses its flags
 * there, and `pnpm agent:fuzz-shrink` restores the env the RECORDING ran under
 * there. A module-level const would capture whatever shell the runner was
 * started from instead - so a trace recorded against one fixture would silently
 * replay against another, and the shrink would report the trace as green. Same
 * rule, and the same reason, as `agentBuildMode()` in `e2e/lib/env.ts`.
 *
 * Knobs only ONE scenario reads (its stream selector, its per-episode sizes)
 * stay in that scenario, next to the code they steer.
 */
import { DEFAULT_FIXTURE } from './fixtures';

/**
 * Replay a recorded trace instead of drawing moves (`STALAGMIGHT_FUZZ_REPLAY`,
 * or `--trace` on `pnpm agent:scenario`, which sets it).
 *
 * Replay is a MODE of a scenario, never a separate one: everything after the
 * move is chosen - the step, the oracles, the model bookkeeping and the failure
 * payload - is the same code on both paths, so a replayed failure is the same
 * failure and not a lookalike. Only the source of the decisions changes.
 */
export function replayTracePath(): string {
  return process.env.STALAGMIGHT_FUZZ_REPLAY ?? '';
}

/** Force-write the trace of a run that PASSED, to this path. */
export function traceOutPath(): string {
  return process.env.STALAGMIGHT_FUZZ_TRACE ?? '';
}

/**
 * Which `.stalx` under `e2e/fixtures/` to fuzz. Basename only - anything else is
 * flattened by `fixtureSourcePath`'s `path.basename`, which is the containment
 * guard and stays the only one.
 */
export function fuzzFixture(): string {
  return process.env.STALAGMIGHT_FUZZ_FIXTURE || DEFAULT_FIXTURE;
}

/**
 * INERT, and kept deliberately. Every issue-413 finding the undo fuzzers
 * reproduced is fixed, every classifier is deleted, and there is nothing left
 * for this flag to tolerate - a mismatch of any shape fails the run.
 *
 * It is still READ, and still reported in the `reproduce:` line, for two
 * reasons: a repro line copied from the issue history still runs (it just no
 * longer changes the verdict), and a failure message that names the flag makes
 * "this run was tolerant" impossible to assume when it was not. If a future
 * open bug ever needs the same escape hatch, re-derive it here - including its
 * rule, which is that it must never be the default, because a green default
 * would mean the oracle had been weakened.
 */
export function allowKnownBugs(): boolean {
  return process.env.STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS === '1';
}

/**
 * `STALAGMIGHT_FUZZ_DUMP_HISTORY=1` prints the WHOLE action history on failure
 * instead of the tail. The tail is right for reading a failure; the full
 * sequence is what you need to transcribe a repro into a Rust test.
 */
export function dumpFullHistory(): boolean {
  return process.env.STALAGMIGHT_FUZZ_DUMP_HISTORY === '1';
}
