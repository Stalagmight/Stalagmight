/**
 * Journey framework (#466). A Journey is a LINEAR golden flow over the real app
 * with explicit behavioral expectations - unlike a stress Scenario, which is a
 * seeded loop whose gate is "did not crash". Journeys reuse ScenarioContext for
 * every app primitive and add two things:
 *
 * - `expect(label, ok, detail)`: records a pass/fail CHECK instead of throwing,
 *   so one breach does not hide the rest; the runner fails the journey if any
 *   check failed. A journey may still throw for a step that makes continuing
 *   meaningless (e.g. the project would not load).
 * - `checkpoint(name)`: drops a screenshot into the session artifacts as
 *   EVIDENCE for the verdict record. Checkpoints never gate - store and disk
 *   state do (no pixel-golden flakiness).
 *
 * Journeys must not hardcode expectations derived from the CONTENT of
 * fixtures (`e2e/fixtures/` is a mix of tracked files and user-managed ones,
 * and either can change under a journey): derive the expected values at
 * runtime (e.g. from `parseTml`) or from data the journey itself constructs.
 */
import * as fs from 'fs';
import * as path from 'path';
import { errorMessageOf, shareableFailureText } from '../lib/shareableFailure';
import type { ScenarioContext, ScenarioTarget } from '../scenarios/framework';
import { makeContext } from '../scenarios/framework';

export interface JourneyCheck {
  label: string;
  ok: boolean;
  /** Shown on failure: what was expected vs observed. */
  detail?: string | undefined;
}

export interface JourneyContext extends ScenarioContext {
  /** Record a behavioral check. Returns `ok` so callers can branch on it. */
  expect(label: string, ok: boolean, detail?: string): boolean;
  /** Screenshot evidence into the session artifacts (never gates). */
  checkpoint(name: string): Promise<void>;
  readonly checks: JourneyCheck[];
}

export interface Journey {
  name: string;
  description: string;
  run(ctx: JourneyContext): Promise<void>;
}

export interface JourneyResult {
  journey: string;
  verdict: 'pass' | 'fail';
  durationMs: number;
  checks: JourneyCheck[];
  /** Message of a thrown abort, when the journey did not run to completion. */
  aborted?: string;
}

export function makeJourneyContext(
  target: ScenarioTarget,
  journeyName: string,
  onLog?: (message: string) => void,
): JourneyContext {
  // Journeys are deterministic flows; the seed only feeds helpers that want a
  // ScenarioContext. Fixed at 1 so any randomized helper draw is reproducible.
  const base = makeContext(target, 1, onLog);
  const checks: JourneyCheck[] = [];
  const shotsDir = base.artifactsDir ? path.join(base.artifactsDir, 'journeys', journeyName) : null;
  return {
    ...base,
    checks,
    expect(label, ok, detail) {
      checks.push(ok ? { label, ok } : { label, ok, detail });
      // The detail is what was observed - the app's data or its error - so it
      // ends a `failed:` line, which the runner prints withheld (#576 W33-NF1).
      base.log(`${ok ? 'ok' : 'FAIL'}: ${label}${!ok && detail ? ` - failed: ${detail}` : ''}`);
      return ok;
    },
    async checkpoint(name) {
      if (!shotsDir) return;
      // Checkpoints are evidence and must NEVER gate: a screenshot failure
      // (WebDriver backend quirk, platform permission) is logged, not thrown -
      // letting it escape would abort a behaviorally passing journey.
      try {
        fs.mkdirSync(shotsDir, { recursive: true });
        const file = path.join(shotsDir, `${name}.png`);
        await target.exclusive(() => target.browser.saveScreenshot(file));
        base.log(`checkpoint ${name} -> ${file}`);
      } catch (err) {
        base.log(`checkpoint ${name} (non-gating) failed: ${err instanceof Error ? err.message : String(err)}`);
      }
    },
  };
}

export async function runJourney(
  journey: Journey,
  target: ScenarioTarget,
  onLog?: (message: string) => void,
): Promise<JourneyResult> {
  const ctx = makeJourneyContext(target, journey.name, onLog);
  const start = Date.now();
  let aborted: string | undefined;
  try {
    await journey.run(ctx);
  } catch (err) {
    // A third-party error's first line is marked as one (#576 W58 Q3): the
    // verdict line and the ledger print it as its length, run.jsonl keeps it.
    aborted = errorMessageOf(err);
  }
  // A journey that recorded no checks asserted nothing - a silent PASS from a
  // spec whose early guards all bailed is exactly the vacuous-test defect the
  // review contract exists to catch. Fail it.
  if (aborted === undefined && ctx.checks.length === 0) {
    aborted = 'journey recorded zero checks - a journey must assert something';
  }
  const failed = aborted !== undefined || ctx.checks.some((c) => !c.ok);
  return {
    journey: journey.name,
    verdict: failed ? 'fail' : 'pass',
    durationMs: Date.now() - start,
    checks: ctx.checks,
    ...(aborted !== undefined ? { aborted } : {}),
  };
}

/** Where a journey's abort reason is kept whole: the run record, not for sharing. */
const ABORT_WITHHELD = "the full reason is in the session's uat/run.jsonl";

/**
 * A journey's abort reason as what a run shares carries it (#576 W33-NF1,
 * W39-NF1): the headline form (`<what> failed: <the app's error>` and `ipc
 * <command>: ...` keep the prefix and the length of the app's text), then one
 * count line for each run of further lines. The verdict line prints it, and the
 * committed UAT record keeps it.
 */
export function shareableAbortReason(aborted: string): string {
  return shareableFailureText(aborted, [], ABORT_WITHHELD);
}

/**
 * The PASS/FAIL line `agent:journeys` prints for one journey (#576 W33-NF1):
 * the check counts, the abort reason in the shareable headline form (`<what>
 * failed: <the app's error>` and `ipc <command>: ...` keep the prefix and the
 * length of the app's text; further lines of the reason are counted on the
 * lines after), then `sweep`: the runner's ` sweep=<counts>` part, the
 * sweep's breaches as `findingsSummary` counts them, or '' when none.
 */
export function journeyVerdictLine(
  verdict: 'pass' | 'fail',
  result: JourneyResult,
  sweep = '',
): string {
  const passed = result.checks.filter((c) => c.ok).length;
  const [reason = '', ...withheld] =
    result.aborted !== undefined ? shareableAbortReason(result.aborted).split('\n') : [];
  return (
    `${verdict.toUpperCase()}: ${result.journey} (${result.durationMs}ms, ${passed}/${result.checks.length} checks)` +
    `${result.aborted ? ` aborted: ${reason}` : ''}` +
    `${sweep}\n` +
    withheld.map((line) => `${line}\n`).join('')
  );
}
