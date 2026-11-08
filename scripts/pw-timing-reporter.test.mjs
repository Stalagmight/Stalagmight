// Fixture tests for the @smoke timing reporter (scripts/pw-timing-reporter.mjs).
//
// The reporter turns Playwright's step tree into the hook-vs-body split that the
// pare-down plan rests on. Two attribution bugs here would each have produced a
// confident wrong answer rather than an error, so both are pinned below with the
// step shapes Playwright actually emits.
//
// Pure Node, no test framework (mirrors scripts/check-no-em-dash.test.mjs).
// Run: node scripts/pw-timing-reporter.test.mjs

import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import TimingReporter from './pw-timing-reporter.mjs';

let passed = 0;
const failures = [];
function test(name, fn) {
  try {
    fn();
    passed++;
  } catch (e) {
    failures.push(`${name}: ${e.message}`);
  }
}

const dir = mkdtempSync(join(tmpdir(), 'pw-timing-'));
let seq = 0;

const mkTest = (file, line, title) => ({ id: `t${seq++}`, location: { file, line }, title });
const step = (title, duration, parent) => ({ category: 'hook', title, duration, parent });

/** Drive a reporter through one test's lifecycle and return the written report. */
function run(steps, { status = 'passed', duration = 1000, retry = 0, test: t } = {}) {
  const out = join(dir, `r${seq++}.json`);
  process.env.PW_TIMING_OUTPUT = out;
  const r = new TimingReporter();
  r.onBegin();
  const subject = t ?? mkTest('/repo/e2e-playwright/a.spec.ts', 1, 'x');
  const result = { retry, status, duration };
  for (const s of steps) r.onStepEnd(subject, result, s);
  r.onTestEnd(subject, result);
  r.onEnd({ status: 'passed', duration: 1234 });
  return JSON.parse(readFileSync(out, 'utf8'));
}

// --- hook attribution ----------------------------------------------------------

test('Before Hooks and After Hooks land in their own buckets', () => {
  const rep = run([step('Before Hooks', 400), step('After Hooks', 60)]);
  assert.equal(rep.tests[0].before, 400);
  assert.equal(rep.tests[0].after, 60);
});

test('Worker Cleanup is NOT counted as setup', () => {
  // Playwright emits "Worker Cleanup" as a SIBLING of Before/After Hooks and
  // charges it to whichever test ran last on that worker. Counting it as
  // `before` inflated the last test of every worker and every test next to a
  // failure - precisely the rows a cost investigation stares at.
  const rep = run([step('Before Hooks', 100), step('Worker Cleanup', 5000)]);
  assert.equal(rep.tests[0].before, 100, 'worker cleanup leaked into setup time');
  assert.equal(rep.tests[0].worker, 5000, 'worker cleanup should still be recorded, just separately');
});

test('hook time cannot exceed the test duration once Worker Cleanup is separated', () => {
  const rep = run([step('Before Hooks', 300), step('Worker Cleanup', 9000)], { duration: 1000 });
  const t = rep.tests[0];
  assert.ok(t.before + t.after <= t.duration, `${t.before}+${t.after} > ${t.duration}`);
});

test('nested hook steps are not double-counted', () => {
  const parent = step('Before Hooks', 500);
  const rep = run([parent, step('beforeEach hook', 480, parent)]);
  assert.equal(rep.tests[0].before, 500);
});

test('non-hook steps are ignored entirely', () => {
  const rep = run([{ category: 'expect', title: 'expect.toBeVisible', duration: 900 }]);
  assert.equal(rep.tests[0].before, 0);
  assert.equal(rep.tests[0].after, 0);
});

// --- identity ------------------------------------------------------------------

test('the recorded file keeps its directory, so same-named specs cannot merge', () => {
  // The merge key is built from this field. A bare basename would silently fuse
  // two different specs that share a name and a line number.
  const rep = run([step('Before Hooks', 10)], {
    test: mkTest(join(process.cwd(), 'e2e-playwright/sub/a.spec.ts'), 1, 'x'),
  });
  assert.match(rep.tests[0].file, /sub\/a\.spec\.ts$/);
});

// --- status --------------------------------------------------------------------

test('a non-passing status is recorded and flagged', () => {
  const rep = run([step('Before Hooks', 10)], { status: 'timedOut' });
  assert.equal(rep.tests[0].status, 'timedOut');
  assert.equal(rep.tests[0].everFailed, true);
});

test('a clean pass is not flagged', () => {
  const rep = run([step('Before Hooks', 10)], { status: 'passed' });
  assert.equal(rep.tests[0].everFailed, false);
});

test('retries are collapsed into one row charging every attempt, and keep the failure on record', () => {
  const out = join(dir, `retry.json`);
  process.env.PW_TIMING_OUTPUT = out;
  const r = new TimingReporter();
  r.onBegin();
  const t = mkTest('/repo/e2e-playwright/a.spec.ts', 7, 'flaky');
  for (const [retry, status, duration] of [
    [0, 'failed', 700],
    [1, 'passed', 300],
  ]) {
    const result = { retry, status, duration };
    r.onStepEnd(t, result, step('Before Hooks', 100));
    r.onTestEnd(t, result);
  }
  r.onEnd({ status: 'passed', duration: 1 });
  const rep = JSON.parse(readFileSync(out, 'utf8'));
  assert.equal(rep.tests.length, 1, 'retries should collapse to one row');
  assert.equal(rep.tests[0].duration, 1000, 'all attempts should be charged');
  assert.equal(rep.tests[0].attempts, 2);
  assert.equal(rep.tests[0].status, 'passed', 'last attempt wins');
  assert.equal(rep.tests[0].everFailed, true, 'the failed attempt must stay on record');
});

test('an unrecognized top-level hook step is NOT silently absorbed as setup', () => {
  // The bare-else version of this dispatch is how "Worker Cleanup" became setup
  // time in the first place. A future fourth hook title must not repeat it.
  const rep = run([step('Before Hooks', 100), step('Teardown Something New', 4000)]);
  assert.equal(rep.tests[0].before, 100, 'unknown hook step leaked into setup time');
  assert.equal(rep.tests[0].unknown, 4000, 'unknown hook time should still be recorded');
});

test('the merged row carries no stale retry index', () => {
  // `retry` used to freeze to the first attempt while `attempts` counted them
  // all, so an artifact reader saw retry:0 on a test that plainly retried.
  const rep = run([step('Before Hooks', 10)]);
  assert.ok(!('retry' in rep.tests[0]), 'retry should not be persisted; attempts carries it');
});

// --- output shape --------------------------------------------------------------

test('a missing step duration is treated as zero, not NaN', () => {
  const rep = run([step('Before Hooks', undefined)]);
  assert.equal(rep.tests[0].before, 0);
});

test('stats carry the run count', () => {
  const rep = run([step('Before Hooks', 10)]);
  assert.equal(rep.stats.tests, 1);
});

// stdout names the env path folded: the path is the caller's, and can name
// their own directories (#576 W51, W58).
test('the summary line names the $PW_TIMING_OUTPUT path folded', () => {
  const logged = [];
  const log = console.log;
  const home = process.env.HOME;
  console.log = (...a) => logged.push(a.join(' '));
  // The output path sits under the home directory, so it prints as ~/...
  process.env.HOME = dir;
  try {
    run([]);
  } finally {
    console.log = log;
    if (home === undefined) delete process.env.HOME;
    else process.env.HOME = home;
  }
  assert.deepEqual(logged, [`\n[pw-timing] wrote 1 test timings to ~/${basename(process.env.PW_TIMING_OUTPUT)}`]);
  assert.ok(!logged[0].includes(dir), 'the absolute path must not print');
});

// The reports above were written under `dir`; remove it so a run leaves
// nothing behind in TMPDIR (#576 E18(b)).
rmSync(dir, { recursive: true, force: true });
test('the temp report dir is removed again', () => {
  assert.equal(existsSync(dir), false);
});

if (failures.length > 0) {
  console.error(`pw-timing-reporter tests: ${failures.length} FAILED`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`pw-timing-reporter tests: ${passed} passed`);
