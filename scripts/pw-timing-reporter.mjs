// Playwright reporter that records per-test cost, split into hook vs body time.
//
// Why a custom reporter: the built-in `json` reporter does NOT emit `steps`, so a
// json report can tell you a test took 6s but not whether 5s of that was the
// beforeEach. Splitting those is the whole question when deciding what to keep in
// the @smoke subset - a suite that is slow because of per-test setup should be
// fixed, not cut, because cutting tests does nothing about the setup cost paid by
// the survivors. `onStepEnd` gives us `category: 'hook'` durations.
//
// Usage:
//   pnpm exec playwright test --reporter=./scripts/pw-timing-reporter.mjs
//   (compose with others: --reporter=github,./scripts/pw-timing-reporter.mjs)
//
// Output path comes from PW_TIMING_OUTPUT (default: playwright-report/timing.json).
// Render it with scripts/pw-timing-summary.mjs.

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, relative } from 'node:path';

import { foldLocalPaths } from './lib/fold-local-paths.mjs';

export default class TimingReporter {
  /** Titles already warned about, so an unknown step warns once, not per test. */
  static warned = new Set();

  constructor() {
    /** @type {Map<string, {before:number, after:number}>} keyed by test id + retry */
    this.hooks = new Map();
    this.rows = [];
    this.startedAt = 0;
  }

  onBegin() {
    this.startedAt = Date.now();
  }

  #key(test, result) {
    return `${test.id}#${result.retry}`;
  }

  onStepEnd(test, result, step) {
    if (step.category !== 'hook') return;
    // Only top-level hook steps are counted; nested steps are already inside the
    // parent's duration, so adding them would double-count.
    if (step.parent?.category === 'hook') return;
    // Playwright emits three top-level hook steps: "Before Hooks", "After Hooks",
    // and "Worker Cleanup". The last is a SIBLING of the other two, not a child,
    // and it is charged to whichever test happened to run last on that worker
    // (see workerMain's _runAsStep on the stopped path). It is neither this test's
    // setup nor its teardown, and testInfo.duration excludes it - counting it as
    // `before` inflated the last test of every worker, and every test adjacent to
    // a failure, which is exactly the population a cost investigation looks at.
    // Track it separately so it can never masquerade as per-test setup cost.
    const k = this.#key(test, result);
    const e = this.hooks.get(k) ?? { before: 0, after: 0, worker: 0, unknown: 0 };
    const ms = step.duration ?? 0;
    if (/^Before/i.test(step.title)) e.before += ms;
    else if (/^After/i.test(step.title)) e.after += ms;
    else if (/^Worker/i.test(step.title)) e.worker += ms;
    else {
      // Match the three titles explicitly rather than defaulting the unmatched
      // case into `before`. A future Playwright adding a fourth top-level hook
      // step would otherwise be silently absorbed as setup - reproducing the
      // exact confident-wrong-number failure this reporter was just fixed for.
      e.unknown += ms;
      if (!TimingReporter.warned.has(step.title)) {
        TimingReporter.warned.add(step.title);
        console.warn(
          `[pw-timing] unrecognized top-level hook step ${JSON.stringify(step.title)} - ` +
            'not counted as setup; update scripts/pw-timing-reporter.mjs'
        );
      }
    }
    this.hooks.set(k, e);
  }

  onTestEnd(test, result) {
    const h =
      this.hooks.get(this.#key(test, result)) ?? { before: 0, after: 0, worker: 0, unknown: 0 };
    this.rows.push({
      // Repo-relative, not a basename: two specs in different directories can
      // share a name, and the onEnd merge key is built from this.
      file: relative(process.cwd(), test.location.file),
      line: test.location.line,
      title: test.title,
      duration: result.duration ?? 0,
      before: h.before,
      after: h.after,
      worker: h.worker ?? 0,
      unknown: h.unknown ?? 0,
      status: result.status,
    });
  }

  onEnd(result) {
    // Collapse retries into one row per test, charging all attempts (that is what
    // CI actually spends).
    // `retry` is deliberately absent from the merged row: it would freeze to the
    // first attempt's index while `attempts` counted them all, so an artifact
    // reader would see `retry: 0` on a test that plainly retried. `attempts`
    // carries that information without the contradiction.
    const merged = new Map();
    for (const r of this.rows) {
      const k = `${r.file}:${r.line}:${r.title}`;
      const e =
        merged.get(k) ??
        { ...r, duration: 0, before: 0, after: 0, worker: 0, unknown: 0, attempts: 0 };
      e.duration += r.duration;
      e.before += r.before;
      e.after += r.after;
      e.worker += r.worker;
      e.unknown += r.unknown;
      e.attempts += 1;
      // Last attempt wins, matching how Playwright decides a retried test's fate,
      // but a test that ever failed keeps that on record - a row whose timings are
      // inflated by a failed attempt must not read as a clean, merely-slow test.
      e.status = r.status;
      e.everFailed = (e.everFailed ?? false) || r.status !== 'passed';
      merged.set(k, e);
    }
    const out = {
      stats: {
        duration: result.duration ?? Date.now() - this.startedAt,
        status: result.status,
        tests: merged.size,
      },
      tests: [...merged.values()],
    };
    const path = process.env.PW_TIMING_OUTPUT || 'playwright-report/timing.json';
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, JSON.stringify(out, null, 2));
    // Folded: the env path is the caller's and can name their own directories
    // (#576 W51, W58).
    console.log(`\n[pw-timing] wrote ${merged.size} test timings to ${foldLocalPaths(path)}`);
  }
}
