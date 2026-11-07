/**
 * The UAT run record (#466) and the form of it `agent:journeys -- --ledger`
 * commits to `docs/uat/runs/` (#576 W39-NF1).
 *
 * That directory is in the PUBLIC tree, so the committed record carries codes,
 * counts and code locations only, in the shareable form the rest of the harness
 * prints (`lib/shareableFailure.ts`): a failed check's observed detail - the
 * app's data or its error - as its length, an abort reason in the headline form
 * the verdict line prints, and each sweep finding as its severity, kind and
 * code. It is an ALLOWLIST: a field added to a journey result later stays in the
 * session's verbatim `uat/run.jsonl` until someone adds it here.
 */
import { withheldAppText } from '../lib/shareableFailure';
import type { Finding } from '../lib/oracles';
import { shareableAbortReason, type JourneyCheck, type JourneyResult } from './framework';

export interface JourneyRecord extends JourneyResult {
  sweepFindings: Finding[];
}

export interface RunRecord {
  kind: 'uat-journeys';
  date: string;
  gitSha: string;
  platform: string;
  verdict: 'pass' | 'fail';
  journeys: JourneyRecord[];
}

/** What the committed record says in place of a sweep finding's message. */
const FINDING_MESSAGE = '<scrubbed - see the session run.jsonl>';

function ledgerCheck(check: JourneyCheck): JourneyCheck {
  const { label, ok, detail } = check;
  return detail !== undefined ? { label, ok, detail: withheldAppText(detail) } : { label, ok };
}

function ledgerFinding(finding: Finding): Finding {
  const { severity, kind, code } = finding;
  // `code` is the app's code location (a log target), never text from the data.
  return { severity, kind, ...(code !== undefined ? { code } : {}), message: FINDING_MESSAGE };
}

/** `record` as `docs/uat/runs/` keeps it; the verbatim record goes to the session's `uat/run.jsonl`. */
export function ledgerRecord(record: RunRecord): RunRecord {
  const { kind, date, gitSha, platform, verdict } = record;
  return {
    kind,
    date,
    gitSha,
    platform,
    verdict,
    journeys: record.journeys.map((j) => ({
      journey: j.journey,
      verdict: j.verdict,
      durationMs: j.durationMs,
      checks: j.checks.map(ledgerCheck),
      ...(j.aborted !== undefined ? { aborted: shareableAbortReason(j.aborted) } : {}),
      sweepFindings: j.sweepFindings.map(ledgerFinding),
    })),
  };
}
