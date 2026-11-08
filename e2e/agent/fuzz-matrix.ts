/**
 * Parallel (fixture x seed [x axis]) matrix runner for a fuzz scenario.
 *
 * Three runners, three jobs:
 *   - `pnpm agent:scenarios`   - every scenario once, at small n. A cheap gate.
 *   - `pnpm agent:scenario`    - ONE scenario at ONE seed. A repro, or a soak.
 *   - `pnpm agent:fuzz-matrix` - THIS: one scenario over MANY cells, K live app
 *     sessions at a time, aggregated into one verdict.
 *
 * Reach for it when the question is coverage rather than a single answer: "does
 * the undo/redo model survive seeds 1..8 on both fixtures?", or a sweep after
 * touching an undo path. Every cell is a CHILD PROCESS of `scenario-run.ts`, so
 * a cell that wedges, crashes the app or leaks a driver cannot take the matrix
 * down with it, and each cell gets its own app session, profile, Xvfb display
 * and port block.
 *
 *   pnpm agent:fuzz-matrix [--scenario NAME] [--fixtures a.stalx,b.stalx]
 *                          [--seeds 1..8|1,3,7] [--axis KEY=v1,v2]
 *                          [--workers K] [--iterations M]
 *
 * `--scenario` defaults to `undo-redo-fuzz`, so every existing invocation means
 * exactly what it did. It is validated by SHAPE only, never against the registry:
 * importing `./scenarios` here would evaluate the fuzz scenarios' module scope
 * in the PARENT process, and `scenario-run.ts` documents at length why that
 * import has to happen after the environment is settled. The child rejects an
 * unknown name with the known-scenario list anyway.
 *
 * `--axis KEY=v1,v2,...` is a SECOND matrix dimension, orthogonal to
 * `--fixtures`: each cell runs with `KEY` set to its value in the child's
 * environment. It is how a knob gets swept - `--axis
 * STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P=0,0.5,1` - without a wrapper script per
 * value. Only `STALAGMIGHT_*` keys are accepted: this process spreads the whole
 * environment into every child, which then invokes cargo and launches the app,
 * so a matrix flag does not get to set `PATH`, `NODE_OPTIONS` or `LD_PRELOAD` -
 * the same boundary `fuzzTrace.ts`'s `restoreTraceEnv` draws for a trace file.
 * The keys the matrix assigns ITSELF are refused too (`RESERVED_AXIS_KEYS` in
 * `lib/fuzzMatrixArgs.ts`): an axis over the per-cell fixture, port block or
 * display would mislabel the report or collapse the workers onto one session.
 *
 * Every `STALAGMIGHT_*` variable reaches the children unchanged, including
 * `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS` - which is now INERT: every issue-413
 * finding `undo-redo-fuzz` reproduced is fixed and every classifier deleted, so
 * a matrix run is expected GREEN on every cell and there is nothing to tolerate.
 * Exits non-zero if any cell failed. Run on the repo-pinned Node (`nvm use`);
 * the children need Xvfb.
 */
import { spawn, type ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { checkBuildFreshness } from './lib/build';
import { agentBuildMode } from '../lib/env';
import {
  cellVerdict,
  failHeadline,
  failureDetail,
  matrixSummary,
  parseMetrics,
  type CellVerdict,
} from './lib/fuzzMatrixReport';
import { RUN_START_ENV, runStartMs } from './lib/recorder';
import { foldPrivateFixtures, shareableFixtureName } from './lib/privateFixtures';
import { foldLocalPaths, shareableErrorText, shareableFailureText } from './lib/shareableFailure';
import {
  BASE_PORT,
  INHERITED_ENV_BLOCKLIST,
  MatrixArgError,
  PORT_STRIDE,
  TOP_DISPLAY,
  parseArgs,
  slotEnv,
  type AxisValue,
  type Options,
} from './lib/fuzzMatrixArgs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
/** The repo's own tsx, so no pnpm wrapper sits between us and the child. */
const TSX_BIN = path.join(repoRoot, 'node_modules', '.bin', 'tsx');
const CHILD_SCRIPT = path.join('e2e', 'agent', 'scenario-run.ts');
const CHILD_TIMEOUT_MS = 20 * 60 * 1000;
/**
 * Metrics that are per-run MAXIMA, not counters: merging them across the matrix
 * takes the max. Everything else is a tally (`op:createShot`, `mutations`,
 * `knownAsymmetry`, ...) and sums. A per-run constant such as `historyLimit`
 * still sums, so it reads as cells * constant - that is the documented rule.
 */
const MAX_MERGED_METRICS = new Set([
  'snapshotBytesMax',
  'maxDepth',
  // `import-dialog-fuzz`'s in-page call maxima. Summing them would report a
  // matrix of 30 cells as having spent 30x its worst call, which is exactly the
  // number nobody wants: the question these answer is how close ONE call came to
  // WebDriver's 30 s `execute/async` ceiling.
  'bridgeStepMsMax',
  'bridgeDigestMsMax',
  'canonicalMsMax',
  'canonicalBytesMax',
]);

const USAGE =
  'usage: pnpm agent:fuzz-matrix [--scenario NAME] [--fixtures a.stalx,b.stalx] [--seeds 1..8|1,3,7] ' +
  '[--axis STALAGMIGHT_KEY=v1,v2] [--workers K] [--iterations M] [--release]';

interface RunResult {
  fixture: string;
  seed: number;
  axis: AxisValue | null;
  /** `lib/fuzzMatrixReport.ts` `cellVerdict`: a LOST cell reached no verdict at all. */
  verdict: CellVerdict;
  passed: boolean;
  durationMs: number;
  metrics: Record<string, number>;
  // FAIL only: the error's first line, the scenario's own `reproduce:` line
  // verbatim, and everything else worth keeping - see `lib/fuzzMatrixReport.ts`,
  // which owns what "worth keeping" means and why the choice matters.
  headline: string;
  reproduce: string;
  detail: string;
}

/**
 * In the crash form (#576 W51): the headline as a `fatal:` line prints it,
 * folded, and any further line counted - a message can carry a path the user gave.
 */
function bail(message: string): never {
  process.stderr.write(`${shareableFailureText(message, [], 'the rest of the message is not printed')}\n${USAGE}\n`);
  process.exit(2);
}

/** `KEY=value`, folded (#576 W51): an axis value is the user's, and a knob can take a path. */
function axisLabel(axis: AxisValue | null): string {
  return axis ? foldLocalPaths(`${axis.key}=${axis.value}`) : '';
}

const live = new Set<ChildProcess>();

function killGroup(child: ChildProcess, signal: NodeJS.Signals): void {
  if (!child.pid) return;
  try {
    // Children are spawned detached, so each leads its own group: one signal
    // reaps tsx + tauri-driver + WebKitWebDriver + the app together.
    process.kill(-child.pid, signal);
  } catch {
    // Group already gone.
  }
}

function runOne(
  cell: { fixture: string; seed: number; axis: AxisValue | null },
  slot: number,
  opts: Options,
  label: string,
): Promise<RunResult> {
  const { fixture, seed, axis } = cell;
  const startedAt = Date.now();
  process.stdout.write(
    `${label} start ${shareableFixtureName(fixture)} seed=${seed}${axis ? ` ${axisLabel(axis)}` : ''} slot=${slot} ` +
      `port=${BASE_PORT + slot * PORT_STRIDE} display=:${TOP_DISPLAY - slot}\n`,
  );

  return new Promise<RunResult>((resolve) => {
    const child = spawn(
      TSX_BIN,
      [CHILD_SCRIPT, opts.scenario, '--seed', String(seed), '--iterations', String(opts.iterations)],
      {
        cwd: repoRoot,
        detached: true,
        stdio: ['ignore', 'pipe', 'pipe'],
        env: slotEnv(slot, fixture, opts.release, axis),
      },
    );
    live.add(child);
    const stdout: string[] = [];
    const stderr: string[] = [];
    child.stdout?.on('data', (chunk: Buffer) => stdout.push(chunk.toString()));
    child.stderr?.on('data', (chunk: Buffer) => stderr.push(chunk.toString()));

    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      killGroup(child, 'SIGTERM');
      setTimeout(() => killGroup(child, 'SIGKILL'), 5000).unref();
    }, CHILD_TIMEOUT_MS);

    // 'error' (spawn failure) and 'close' both settle: a child that emits only
    // 'error' would otherwise hang its worker slot for the life of the run.
    let settled = false;
    const finish = (code: number | null, signal: NodeJS.Signals | null, spawnError?: Error): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      live.delete(child);
      const outLines = stdout.join('').split('\n');
      const allLines = outLines.concat(stderr.join('').split('\n'));
      const passLine = outLines.find((line) => line.startsWith('PASS: '));
      const failIndex = outLines.findIndex((line) => line.startsWith('FAIL: '));
      const verdict = cellVerdict({
        spawnFailed: spawnError !== undefined,
        timedOut,
        code,
        sawPass: passLine !== undefined,
        sawFail: failIndex >= 0,
      });
      const passed = verdict === 'pass';

      let headline = '';
      // Folded (#576 W51): a spawn error names the tsx binary in the checkout.
      if (spawnError) headline = `spawn failed: ${foldLocalPaths(spawnError.message)}`;
      else if (timedOut) headline = `timed out after ${Math.round(CHILD_TIMEOUT_MS / 60000)}min (process group killed)`;
      else if (!passed && failIndex >= 0) {
        headline = failHeadline(outLines, failIndex);
      } else if (!passed) headline = `child exited code=${code} signal=${signal ?? 'none'} with no FAIL line`;
      if (verdict === 'lost') headline = `LOST (no verdict): ${headline}`;

      resolve({
        fixture,
        seed,
        axis,
        verdict,
        passed,
        durationMs: Date.now() - startedAt,
        metrics: passLine ? parseMetrics(passLine) : {},
        headline,
        reproduce: passed ? '' : (allLines.find((line) => line.trim().startsWith('reproduce:')) ?? '').trim(),
        detail: passed ? '' : failureDetail({ outLines, allLines }),
      });
    };
    child.on('error', (err: Error) => finish(null, null, err));
    child.on('close', (code, signal) => finish(code, signal));
  });
}

function secs(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}

function printDone(result: RunResult, label: string): void {
  process.stdout.write(
    `${label} ${result.verdict === 'lost' ? 'LOST ' : result.passed ? 'PASS ' : 'FAIL '} ${shareableFixtureName(result.fixture)} seed=${result.seed}` +
      `${result.axis ? ` ${axisLabel(result.axis)}` : ''} ` +
      `(${secs(result.durationMs)})${result.passed ? '' : ` ${result.headline}`}\n`,
  );
}

function printReport(results: RunResult[], opts: Options, wallMs: number): void {
  const withAxis = results.some((r) => r.axis !== null);
  const rows = results.map((r) =>
    [
      shareableFixtureName(r.fixture),
      String(r.seed),
      ...(withAxis ? [r.axis ? foldLocalPaths(r.axis.value) : ''] : []),
      r.verdict === 'lost' ? 'LOST' : r.passed ? 'PASS' : 'FAIL',
      secs(r.durationMs),
      r.headline,
    ],
  );
  const axisKey = results.find((r) => r.axis !== null)?.axis?.key ?? 'axis';
  const header = ['fixture', 'seed', ...(withAxis ? [axisKey] : []), 'result', 'duration', 'detail'];
  // Every row has one cell per header column (both split on the same withAxis),
  // so `row[i]` and `widths[i]` are in range.
  const widths = header.map((h, i) => Math.max(h.length, ...rows.map((row) => row[i]!.length)));
  const line = (cells: string[]): string =>
    cells.map((cell, i) => (i === cells.length - 1 ? cell : cell.padEnd(widths[i]!))).join('  ').trimEnd();

  process.stdout.write(`\n=== fuzz matrix: ${results.length} cell(s) ===\n${line(header)}\n`);
  for (const row of rows) process.stdout.write(`${line(row)}\n`);

  const failures = results.filter((r) => !r.passed);
  if (failures.length) {
    process.stdout.write('\n=== repro commands ===\n');
    for (const failure of failures) {
      // Folded (#576 W51): a `reproduce:` line Node printed itself in an
      // uncaught error can name a path under the home directory. A fixture the
      // repo does not track prints as its tag (#576 W58 Q1).
      const fixture = shareableFixtureName(failure.fixture);
      const prefix = `STALAGMIGHT_FUZZ_FIXTURE=${fixture}${failure.axis ? ` ${axisLabel(failure.axis)}` : ''}`;
      process.stdout.write(
        `${fixture} seed=${failure.seed}${failure.axis ? ` ${axisLabel(failure.axis)}` : ''}: ` +
          `${foldPrivateFixtures(foldLocalPaths(failure.reproduce)) || `(no reproduce line) ${prefix} pnpm agent:scenario ${opts.scenario} --seed ${failure.seed}`}\n`,
      );
      // ALWAYS, not just when there is no repro line. A matrix cell's child
      // process and its session are gone by the time anyone reads this, so a
      // payload that is not printed here is destroyed - and the earlier
      // condition dropped it for precisely the failures that HAD one, since
      // every oracle failure carries a `reproduce:` line and only a bare crash
      // does not. Two reds of the 130-cell corpus campaign had to be diagnosed
      // from their solo-retry logs for that reason, and one - a first-run-only
      // S4 - could not be diagnosed at all.
      if (failure.detail) {
        process.stdout.write(`${failure.detail.split('\n').map((l) => `    | ${l}`).join('\n')}\n`);
      }
    }
  }

  const merged: Record<string, number> = {};
  for (const result of results) {
    for (const [key, value] of Object.entries(result.metrics)) {
      merged[key] = MAX_MERGED_METRICS.has(key)
        ? Math.max(merged[key] ?? Number.NEGATIVE_INFINITY, value)
        : (merged[key] ?? 0) + value;
    }
  }
  // GENERATED from the set, and from the keys actually present, so it cannot go
  // stale the way a hand-written list did: the legend named `snapshotBytesMax`
  // and `maxDepth` long after four more maxima joined them, which labelled real
  // maxima as sums in every campaign summary that printed them. Naming only the
  // ones this run produced also keeps the line honest about the run in hand.
  const maxed = Object.keys(merged)
    .filter((key) => MAX_MERGED_METRICS.has(key))
    .sort();
  process.stdout.write(
    `\n=== merged metrics (${maxed.length > 0 ? `max for ${maxed.join(', ')}; ` : ''}sum otherwise) ===\n`,
  );
  for (const key of Object.keys(merged).sort()) process.stdout.write(`  ${key} = ${merged[key]}\n`);

  process.stdout.write(`\n${matrixSummary(results, wallMs).join('\n')}\n`);
}

/**
 * Parse, turning `MatrixArgError` back into the CLI's own "message, usage,
 * exit 2". The parser throws rather than exiting so it can be unit-tested;
 * this is the one place that difference is undone.
 */
function parseOrBail(argv: string[]): Options {
  try {
    return parseArgs(argv);
  } catch (err) {
    if (err instanceof MatrixArgError) bail(err.message);
    throw err;
  }
}

async function main(): Promise<void> {
  const opts = parseOrBail(process.argv.slice(2));
  for (const note of opts.notes) process.stdout.write(`${note}\n`);
  if (!fs.existsSync(TSX_BIN)) bail(`tsx not found at ${foldLocalPaths(TSX_BIN)}; run pnpm install`);
  const dropped = INHERITED_ENV_BLOCKLIST.filter((key) => process.env[key] !== undefined);
  if (dropped.length > 0) {
    process.stdout.write(
      `note: ${dropped.join(', ')} is set but NOT passed to the cells - a matrix run generates its ` +
        `own moves. Use pnpm agent:scenario --trace to replay one.\n`,
    );
  }
  const cells = opts.fixtures.flatMap((fixture) =>
    opts.seeds.flatMap((seed) => opts.axis.map((axis) => ({ fixture, seed, axis }))),
  );
  const workers = Math.min(opts.workers, cells.length);
  const axisKey = opts.axis.find((entry) => entry !== null)?.key;
  process.stdout.write(
    `${opts.scenario}: ${cells.length} cell(s) = ${opts.fixtures.length} fixture(s) x ${opts.seeds.length} seed(s)` +
      `${axisKey ? ` x ${opts.axis.length} ${axisKey} value(s)` : ''}, ` +
      `iterations=${opts.iterations}, workers=${workers}, ` +
      `mode=${opts.release ? 'release' : agentBuildMode()}\n`,
  );

  process.on('SIGINT', () => {
    process.stdout.write(`\ninterrupted: killing ${live.size} live child run(s)\n`);
    for (const child of live) killGroup(child, 'SIGTERM');
    setTimeout(() => {
      for (const child of live) killGroup(child, 'SIGKILL');
      process.exit(130);
    }, 2000);
  });

  const startedAt = Date.now();
  // Session retention reads "this run" as the whole MATRIX, not one cell: every
  // cell inherits this, so a trace a finished cell left behind outlives the
  // cells that start after it (#576 HFI-6, `recorder.ts` `pruneProtection`).
  process.env[RUN_START_ENV] = String(Math.floor(runStartMs()));
  const results: RunResult[] = [];
  // WARM-UP: from a cold tree the first cell runs ALONE, because each child does
  // its own `build: 'auto'` and K concurrent cargo builds would just queue on the
  // target-dir lock (or time out waiting for it). One solo cell pays for the
  // build; the pool then starts against a fresh binary. Skipped when the binary
  // is already fresh - there is nothing to pay for, and serializing a cell would
  // cost a whole app session.
  const freshness = checkBuildFreshness({ mode: opts.release ? 'release' : agentBuildMode() });
  let next = 0;
  if (cells.length > 1 && workers > 1 && !freshness.fresh) {
    const first = cells[0]!;
    next = 1;
    const label = `[1/${cells.length}]`;
    // Folded (#576 W51), as session.ts folds its `Rebuilding:` line: a reason can
    // name the checkout a build stamp came from.
    process.stdout.write(`${label} warm-up alone (rebuild needed: ${foldLocalPaths(freshness.reasons.join('; '))})\n`);
    const warm = await runOne(first, 0, opts, label);
    results.push(warm);
    printDone(warm, label);
  }

  await Promise.all(
    Array.from({ length: workers }, (_unused, slot) =>
      (async () => {
        for (;;) {
          const index = next++;
          if (index >= cells.length) return;
          const cell = cells[index]!;
          const label = `[${index + 1}/${cells.length}]`;
          const result = await runOne(cell, slot, opts, label);
          results.push(result);
          printDone(result, label);
        }
      })(),
    ),
  );

  results.sort(
    (a, b) =>
      a.fixture.localeCompare(b.fixture) ||
      a.seed - b.seed ||
      (a.axis?.value ?? '').localeCompare(b.axis?.value ?? ''),
  );
  printReport(results, opts, Date.now() - startedAt);
  process.exit(results.some((result) => !result.passed) ? 1 : 0);
}

main().catch((err) => {
  for (const child of live) killGroup(child, 'SIGKILL');
  // The shareable crash form (#576 W23-NF2): a crash in session setup can quote
  // the app (an IPC rejection, a page script's error).
  process.stderr.write(`fatal: ${shareableErrorText(err)}\n`);
  process.exit(1);
});
