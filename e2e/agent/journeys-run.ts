/**
 * UAT journey runner (#466). Runs every registered journey against one live
 * app session, with a full oracle sweep after each. Unlike the scenario smoke
 * gate, journeys are BEHAVIORAL: a journey fails on any failed check, any
 * thrown step, or any `error`/`fatal` sweep finding - there is no
 * fatal-only tolerance here, because journeys are curated flows where an error
 * finding means a real breach.
 *
 *   pnpm agent:journeys                 # run all, verdict JSONL into artifacts
 *   pnpm agent:journeys -- --ledger     # additionally append docs/uat/ledger.md
 *                                       # and write the run record to docs/uat/runs/
 *   pnpm agent:journeys -- --only save-reopen-identity
 *   pnpm agent:journeys -- --release    # drive the RELEASE binary (RC checklist runs)
 */
import * as fs from 'fs';
import * as path from 'path';
import { execFileSync } from 'child_process';
import { fileURLToPath } from 'url';
import { createSession } from './createSession';
import { listJourneys, getJourney } from './journeys';
import { runJourney, type JourneyResult } from './journeys/framework';
import { findingsSummary, runSweep, newOracleState, type Finding } from './lib/oracles';
import { shareableErrorText } from './lib/shareableFailure';
import { callStoreAction } from './lib/stores';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

interface JourneyRecord extends JourneyResult {
  sweepFindings: Finding[];
}

interface RunRecord {
  kind: 'uat-journeys';
  date: string;
  gitSha: string;
  platform: string;
  verdict: 'pass' | 'fail';
  journeys: JourneyRecord[];
}

function gitSha(): string {
  try {
    return execFileSync('git', ['-C', repoRoot, 'rev-parse', '--short', 'HEAD'], {
      encoding: 'utf8',
    }).trim();
  } catch {
    return 'unknown';
  }
}

function parseArgs(argv: string[]): { only: string | null; ledger: boolean; release: boolean } {
  let only: string | null = null;
  let ledger = false;
  let release = false;
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--ledger') ledger = true;
    else if (argv[i] === '--release') release = true;
    else if (argv[i] === '--only') only = argv[++i] ?? null;
  }
  return { only, ledger, release };
}

/**
 * The docs/uat/runs/ record is COMMITTED to the public tree, and a failing
 * run's check details and sweep findings carry absolute session paths and raw
 * log lines - content the marker-based `lint:private-leaks` scanner cannot be
 * assumed to catch. Scrub before writing: absolute home paths collapse to
 * their basename, and sweep findings keep only kind + severity. The verbatim
 * record still lands in the session artifacts (`run.jsonl`) for triage.
 */
function sanitizeForLedger(record: RunRecord): RunRecord {
  const scrubText = (text: string): string =>
    text.replace(/\/(?:Users|home)\/[^\s'")\]]+/g, (m) => `<path>/${path.basename(m)}`);
  return {
    ...record,
    journeys: record.journeys.map((j) => ({
      ...j,
      ...(j.aborted !== undefined ? { aborted: scrubText(j.aborted) } : {}),
      checks: j.checks.map((c) =>
        c.detail !== undefined ? { ...c, detail: scrubText(c.detail) } : c,
      ),
      // ALLOWLIST, not a spread: Finding carries a free-form `detail` (crash
      // marker text, error-boundary dumps, leftover-temp paths) that must
      // never reach the committed record.
      sweepFindings: j.sweepFindings.map((f) => ({
        severity: f.severity,
        kind: f.kind,
        message: `<scrubbed - see the session run.jsonl>`,
      })),
    })),
  };
}

function writeLedger(record: RunRecord): void {
  const uatDir = path.join(repoRoot, 'docs', 'uat');
  const runsDir = path.join(uatDir, 'runs');
  fs.mkdirSync(runsDir, { recursive: true });
  const stamp = record.date.replace(/[:]/g, '').slice(0, 17); // YYYY-MM-DDTHHMMSS
  const runFile = path.join(runsDir, `${stamp}-${record.gitSha}.jsonl`);
  // 'wx' so a same-second rerun fails loudly instead of silently replacing the
  // evidence a previously appended ledger line points at (append-only ledger).
  fs.writeFileSync(runFile, `${JSON.stringify(sanitizeForLedger(record))}\n`, { flag: 'wx' });

  const ledgerFile = path.join(uatDir, 'ledger.md');
  const failed = record.journeys.filter((j) => j.verdict === 'fail').map((j) => j.journey);
  const line =
    `- ${record.date.slice(0, 10)} journeys@${record.gitSha} (${record.platform}): ` +
    `**${record.verdict.toUpperCase()}** - ${record.journeys.length} journeys` +
    `${failed.length ? `, failed: ${failed.join(', ')}` : ''} ` +
    `([record](runs/${path.basename(runFile)}))\n`;
  fs.appendFileSync(ledgerFile, line);
  process.stdout.write(`ledger: ${runFile}\n`);
}

async function main(): Promise<void> {
  const { only, ledger, release } = parseArgs(process.argv.slice(2));
  // Same contract as scenario-run.ts: the flag is sugar for the env var the
  // session layer reads. RC checklist runs MUST pass it - the default is the
  // debug binary, which is not the artifact a release signs off.
  if (release) process.env.STALAGMIGHT_AGENT_RELEASE = '1';
  const names = only ? [only] : listJourneys();
  for (const name of names) {
    if (!getJourney(name)) {
      process.stderr.write(`unknown journey "${name}"; known: ${listJourneys().join(', ')}\n`);
      process.exit(2);
    }
  }

  const session = createSession();
  const started = await session.start({ build: 'auto' });
  process.stdout.write(`session ${started.state} (build ${started.buildAction})\n`);
  const oracle = newOracleState();
  const records: JourneyRecord[] = [];

  try {
    for (const name of names) {
      await session
        .exclusive(() => callStoreAction(session.browser, 'project', 'closeProject', []))
        .catch(() => undefined);
      const journey = getJourney(name);
      if (!journey) continue;
      process.stdout.write(`\n== ${name} ==\n`);
      const result = await runJourney(journey, session, (m) => process.stdout.write(`   · ${m}\n`));
      const sweep = await runSweep(session, oracle, { full: true, recorder: session.recorder });
      const breaches = sweep.findings.filter((f) => f.severity === 'fatal' || f.severity === 'error');
      const verdict: 'pass' | 'fail' = result.verdict === 'fail' || breaches.length > 0 ? 'fail' : 'pass';
      records.push({ ...result, verdict, sweepFindings: sweep.findings });
      const failedChecks = result.checks.filter((c) => !c.ok);
      process.stdout.write(
        `${verdict.toUpperCase()}: ${name} (${result.durationMs}ms, ` +
          `${result.checks.length - failedChecks.length}/${result.checks.length} checks)` +
          `${result.aborted ? ` aborted: ${result.aborted}` : ''}` +
          // Counts only (#576 HFI2-F2); the text is in the session's run.jsonl.
          `${breaches.length ? ` sweep=${findingsSummary(breaches)}` : ''}\n`,
      );
    }
  } finally {
    await session.stop('journeys-run');
  }

  const record: RunRecord = {
    kind: 'uat-journeys',
    date: new Date().toISOString(),
    gitSha: gitSha(),
    platform: `${process.platform}-${process.arch}`,
    verdict: records.some((r) => r.verdict === 'fail') ? 'fail' : 'pass',
    journeys: records,
  };

  const artifactsDir = session.sessionDir ? path.join(session.sessionDir, 'uat') : null;
  if (artifactsDir) {
    fs.mkdirSync(artifactsDir, { recursive: true });
    fs.writeFileSync(path.join(artifactsDir, 'run.jsonl'), `${JSON.stringify(record)}\n`);
  }
  if (ledger) writeLedger(record);

  process.stdout.write(`\nJOURNEYS ${record.verdict.toUpperCase()}\n`);
  process.exit(record.verdict === 'fail' ? 1 : 0);
}

main().catch((err) => {
  // The shareable crash form (#576 W23-NF2): a crash in session setup can quote
  // the app (an IPC rejection, a page script's error).
  process.stderr.write(`fatal: ${shareableErrorText(err)}\n`);
  process.exit(1);
});
