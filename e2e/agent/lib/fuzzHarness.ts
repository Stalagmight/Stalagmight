/**
 * Per-run plumbing the four fuzz scenarios used to carry one copy each of:
 * opening a replay trace, the move log a failure message ends on, writing the
 * run's trace, and the shape of the failure itself.
 *
 * `undoRedoFuzz`, `undoPersistenceFuzz`, `exportRoundtripFuzz` and
 * `importDialogFuzz` each had their own, every copy naming another as the place
 * its form came from. The texts, the order of operations and the files written
 * are unchanged; only the home moved.
 *
 * NOTHING HERE DRAWS. No helper takes a PRNG or a `ScenarioContext.rng`, and
 * none of them decides a move: they run after the decision they describe (the
 * log, the trace) or before any move is drawn (the replay check). That is the
 * property every recorded repro line depends on - a seed has to keep producing
 * the same action sequence - and it is why this could be shared at all.
 *
 * The undo-model half the two undo fuzzers share (the fused step, the model and
 * stack assertions, the epoch attribution, the save/reload boundary) is in
 * `undoFuzzHarness.ts`, because it speaks the in-page bridge's protocol and the
 * other two scenarios do not.
 */
import type { ScenarioContext } from '../scenarios/framework';
import { redactCorpusText } from './fixtureCorpus';
import { TraceRecorder, defaultTracePath, readTrace, writeTrace, type FuzzTrace } from './fuzzTrace';
import { replayTracePath, traceOutPath } from './fuzzKnobs';
import { errorMessageOf, foldLocalPaths } from './shareableFailure';

/** What a scenario needs to know about replay, resolved once per run (or cycle). */
export interface ReplaySetup {
  /** `STALAGMIGHT_FUZZ_REPLAY`, or '' when the run draws its own moves. */
  replayPath: string;
  /** `STALAGMIGHT_FUZZ_TRACE`, or '' when a passing run writes no trace. */
  traceOut: string;
  /** The trace being replayed, already checked against this scenario and fixture. */
  replay: FuzzTrace | null;
}

/**
 * Read the replay/record knobs and, when replaying, the trace - refusing one
 * recorded by another scenario or on another fixture.
 *
 * Both refusals matter for the same reason: a trace's steps carry ids and
 * session-relative paths RESOLVED against the recording's project, so replaying
 * them elsewhere drives moves at things that do not exist there - an invented
 * failure, or a green run, either way not the recorded bug.
 *
 * `rerunWith` is the text before `STALAGMIGHT_FUZZ_FIXTURE=` on the fixture
 * refusal's second line. The undo fuzzers explain
 * the ids there and the UI fuzzers do not, and both wordings are kept exactly.
 */
export function openReplay(scenario: string, fixture: string, rerunWith: string): ReplaySetup {
  const replayPath = replayTracePath();
  const traceOut = traceOutPath();
  const replay: FuzzTrace | null = replayPath ? readTrace(replayPath) : null;
  // The trace's own scenario and fixture are read back from a file a forged copy
  // could change, so they go on a later line, which the runners withhold and the
  // session's actions.jsonl keeps (#576 W51).
  if (replay && replay.scenario !== scenario) {
    throw new Error(
      `trace ${foldLocalPaths(replayPath)} was recorded by another scenario; run it with that scenario\n` +
        `  recorded by: ${replay.scenario}`,
    );
  }
  if (replay && replay.fixture !== fixture) {
    throw new Error(
      `trace ${foldLocalPaths(replayPath)} was recorded on another fixture but this run is on ${fixture} ` +
        `(pnpm agent:scenario --trace restores it for you)\n` +
        `  ${rerunWith}STALAGMIGHT_FUZZ_FIXTURE=${replay.fixture}`,
    );
  }
  return { replayPath, traceOut, replay };
}

/** The `rerunWith` the two undo fuzzers use. */
export const RERUN_WITH_IDS = 'its steps carry ids resolved against that project. Re-run with ';
/** The `rerunWith` the import/export UI fuzzers use. */
export const RERUN_WITH = 're-run with ';

/**
 * The run's move history, and the move that is RUNNING RIGHT NOW.
 *
 * A history line reports what a move DID, so it can only be written once the
 * move RETURNS. A move that throws instead - the step never comes back, or an
 * assertion inside it fires - then left the tail ending on the PREVIOUS move,
 * which reads as an attribution and is one: it names the wrong move. The same
 * gotcha in the Rust tier mis-read two of issue #464's defects during triage.
 * So a move is armed before it runs and disarmed after, and `tail` names an
 * armed one. Rendered on failure only, so a run that does not fail behaves
 * exactly as before and its trace is untouched.
 *
 * The three members are arrow properties so a scenario can destructure them and
 * keep calling `note(...)` / `startingMove(...)` / `finishedMove()` bare.
 */
export class MoveLog {
  readonly history: string[] = [];
  private current: string | null = null;

  readonly note = (line: string): void => {
    this.history.push(line);
  };

  readonly startingMove = (label: string): void => {
    this.current = label;
  };

  readonly finishedMove = (): void => {
    this.current = null;
  };

  /** The label of the move that is running right now, or null between moves. */
  inFlight(): string | null {
    return this.current;
  }

  /**
   * The last `limit` lines (every line when `full`), indented for a failure
   * message, plus the armed move if one had not returned.
   */
  tail(limit: number, full = false): string[] {
    const tail = (full ? this.history : this.history.slice(-limit)).map((line) => `    ${line}`);
    if (this.current !== null) tail.push(`    ${this.current} -> IN FLIGHT (had not returned)`);
    return tail;
  }
}

/** What a trace header records about the run, beyond what `ctx` already knows. */
export interface RunTraceMeta {
  scenario: string;
  iterations: number;
  fixture: string;
  stream: string;
  /** Episode indices that ran with no project open (the import fuzzer only). */
  noProjectEpisodes?: readonly number[];
}

/**
 * Write the run's trace, best-effort: a failed write must not mask the failure
 * it was written for, so it answers null instead of throwing.
 *
 * `explicitPath` wins (`STALAGMIGHT_FUZZ_TRACE`); otherwise the trace goes to
 * the session artifacts dir under a timestamped name, and with neither there is
 * nowhere to write it. Returns the path written.
 */
export function writeRunTrace(
  ctx: Pick<ScenarioContext, 'artifactsDir' | 'seed'>,
  recorder: TraceRecorder,
  meta: RunTraceMeta,
  failure: { message: string } | null,
  explicitPath: string,
): string | null {
  const target =
    explicitPath || (ctx.artifactsDir ? defaultTracePath(ctx.artifactsDir, meta.scenario, ctx.seed) : '');
  if (!target) return null;
  try {
    return writeTrace(
      target,
      recorder.build({
        scenario: meta.scenario,
        seed: ctx.seed,
        iterations: meta.iterations,
        fixture: meta.fixture,
        stream: meta.stream,
        failure,
        noProjectEpisodes: meta.noProjectEpisodes,
      }),
    );
  } catch {
    return null;
  }
}

/**
 * A failure message in the shape the undo and import fuzzers share: the
 * headline, the scenario's context lines (the `reproduce:` line first), the
 * detail block when there is one, then `  last N <tailHeading>:` and the tail.
 */
export function failureText(
  headline: string,
  context: readonly string[],
  detail: string,
  tailHeading: string,
  tail: readonly string[],
): string {
  return [headline, ...context, ...(detail ? [detail] : []), `  last ${tail.length} ${tailHeading}:`, ...tail].join(
    '\n',
  );
}

/**
 * Throw `message`, after handing it to `dumpTrace`.
 *
 * The trace carries the message VERBATIM, so a shrink compares signatures taken
 * from the same text a human reads. The trace path is appended afterwards,
 * which is why it is not part of what was recorded.
 */
export function throwFailure(message: string, dumpTrace: (failure: { message: string }) => string | null): never {
  const written = dumpTrace({ message });
  throw new Error(written ? `${message}\n  trace: ${written}` : message);
}

/**
 * Did a WebDriver call outrun its budget - the PAGE stopped answering, rather
 * than the app saying something?
 *
 * Three spellings of one event: webdriver's own request timeout (`The operation
 * was aborted due to timeout when running "execute/async" ...`, the 30 s
 * `connectionRetryTimeout` in `sessionShared.ts`), undici's connect timeout as
 * webdriver rewords it (`Request timed out!`), and the W3C `script timeout`
 * error a driver returns when the in-page script itself overran.
 */
export function isWebDriverStall(err: unknown): boolean {
  const text = err instanceof Error ? `${err.name}: ${err.message}` : String(err);
  return /aborted due to timeout|Request timed out!|script timeout|\bTimeoutError\b/i.test(text);
}

/** A run that died outside its own `fail`, reduced to what `fail` needs. */
export interface RunErrorReport {
  /** True when the page stopped answering (`isWebDriverStall`). */
  stall: boolean;
  /** The failure's first line - what `failureSignature` and a shrink key on. */
  headline: string;
  detail: string;
}

/**
 * Turn an error a scenario did NOT raise through its own `fail` into a failure
 * it can raise: a headline that says what happened and a detail that names the
 * move that had not returned.
 *
 * A fused UI step that outruns the WebDriver budget used to surface as a bare
 * `WebDriverError: The operation was aborted due to timeout` - no move, no
 * `reproduce:` line, no trace - because only oracle failures went through
 * `fail` (#576 HFI-9, 24+ import-dialog runs on one campaign). A STALL headline
 * names the WebDriver command and nothing that varies run to run, so every
 * stall of one kind shrinks against one signature; the `error:` line keeps the
 * driver's own words. Anything else keeps its own first line as the headline,
 * because that line already says what it is.
 */
export function describeRunError(err: unknown, inFlight: string | null): RunErrorReport {
  const message = err instanceof Error ? err.message : String(err);
  const lines = message.split('\n');
  const first = (lines[0] ?? '').trim();
  if (isWebDriverStall(err)) {
    const command = /when running "([^"]+)"/.exec(message)?.[1] ?? 'a WebDriver call';
    // Folded BEFORE the cut (#576 RV18-W23-6): the in-page script can quote a
    // corpus copy, and a name cut in two is a stub no later fold recognises.
    const error = redactCorpusText(first);
    return {
      stall: true,
      headline: `STALL: ${command} did not return within the WebDriver budget - the page stopped answering`,
      detail: [
        `  stalled move: ${inFlight ?? '(none in flight - the stall came between moves)'}`,
        // Bounded: webdriver appends the in-page script to the message.
        `  error: ${error.length > 240 ? `${error.slice(0, 240)}...` : error}`,
      ].join('\n'),
    };
  }
  // A third-party error's first line - webdriverio's, Node's - is not a shape
  // the harness writes: it goes in behind `a third-party <Name>: `, which the
  // shareable form prints as that prefix and the line's length (#576 W58 Q3).
  // The failure message, and so the session's actions.jsonl and the in-memory
  // signature a shrink compares, keeps the line whole.
  const [shown = ''] = errorMessageOf(err).split('\n');
  return {
    stall: false,
    headline: first === '' ? 'the run threw an error with no message' : shown.trim(),
    detail: [`  in flight: ${inFlight ?? '(none)'}`, ...lines.slice(1)].join('\n').trimEnd(),
  };
}
