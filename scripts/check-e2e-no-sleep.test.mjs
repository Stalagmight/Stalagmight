// Fixture tests for the Playwright no-unannotated-sleep gate (scripts/check-e2e-no-sleep.mjs).
//
// Pins what counts as a sleep, what counts as an annotation, and that prose,
// strings and in-page timers do not count. Pure Node, no test framework
// (mirrors scripts/check-e2e-parallel-safe.test.mjs).
// Run: node scripts/check-e2e-no-sleep.test.mjs  (also a CI smoke step).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { findSleeps, settleReason, scan, main, MIN_REASON } from './check-e2e-no-sleep.mjs';

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

const bad = (text) => scan(['e2e-playwright/x.spec.ts'], () => text).violations.map((v) => v.lineNo);

test('an unannotated waitForTimeout fails, with its line', () => {
  assert.deepEqual(bad(["await page.click('#a');", 'await page.waitForTimeout(300);'].join('\n')), [2]);
});

test('every call counts, including two on one line and a non-page receiver', () => {
  assert.deepEqual(bad('await page.waitForTimeout(1); await frame.waitForTimeout(2);'), [1, 1]);
  assert.deepEqual(bad('await p.waitForTimeout (5)'), [1]);
});

test('a "// settle: <reason>" block directly above passes, even as the first line of a longer comment', () => {
  const one = ['// settle: proves the tool does NOT change back', 'await page.waitForTimeout(200);'].join('\n');
  assert.deepEqual(bad(one), []);
  const multi = ['    // settle: proves no dialog opens; its absence', '    // holds on the first read either way.', '    await page.waitForTimeout(300);'].join('\n');
  assert.deepEqual(bad(multi), []);
  assert.equal(settleReason(multi.split('\n'), 2), 'proves no dialog opens; its absence');
});

test('an annotation that is too short, misspelled, or not directly above does NOT count', () => {
  assert.deepEqual(bad(['// settle: wait', 'await page.waitForTimeout(200);'].join('\n')), [2], `reason < ${MIN_REASON} chars`);
  assert.deepEqual(bad(['// Settle first: the compat mousedown arrives later', 'await page.waitForTimeout(200);'].join('\n')), [2]);
  assert.deepEqual(bad(['// settle: proves nothing changes here', '', 'await page.waitForTimeout(200);'].join('\n')), [3], 'a blank line ends the block');
  assert.deepEqual(bad(['// settle: proves nothing changes here', 'await page.click("#a");', 'await page.waitForTimeout(200);'].join('\n')), [3]);
  assert.deepEqual(bad('await page.waitForTimeout(200); // settle: trailing comments are not the contract'), [1]);
});

test('a waitForTimeout in a comment or a string is not a call', () => {
  assert.deepEqual(bad('// Use polling assertion instead of waitForTimeout'), []);
  assert.deepEqual(bad(['/**', ' * Helper to poll (more stable than waitForTimeout(100)).', ' */'].join('\n')), []);
  assert.deepEqual(bad("const label = 'page.waitForTimeout(100)';"), []);
});

test('in-page timers and the test budget are out of scope', () => {
  assert.deepEqual(bad('await page.evaluate(() => new Promise((r) => setTimeout(r, 100)));'), []);
  assert.deepEqual(bad('test.setTimeout(60000);'), []);
});

// #576 RV3-FE-12: each of these is a sleep the gate used to miss.
test('an aliased, bracketed, destructured or split waitForTimeout counts', () => {
  assert.deepEqual(bad('const nap = page.waitForTimeout.bind(page);\nawait nap(500);'), [1]);
  assert.deepEqual(bad("await page['waitForTimeout'](500);"), [1]);
  assert.deepEqual(bad('const { waitForTimeout } = page;'), [1]);
  assert.deepEqual(bad('await page\n  .waitForTimeout\n  (500);'), [2]);
});

test('a test-runner setTimeout or a timers/promises import counts; a settle annotation still clears it', () => {
  assert.deepEqual(bad('await new Promise((r) => setTimeout(r, 500));'), [1]);
  assert.deepEqual(bad('await new Promise((r) =>\n  globalThis.setTimeout(r, 500));'), [2]);
  assert.deepEqual(bad("import { setTimeout as sleep } from 'node:timers/promises';\nawait sleep(500);"), [1]);
  assert.deepEqual(bad("const { setTimeout: nap } = require('timers/promises');"), [1]);
  assert.deepEqual(bad('// settle: proves the debounce never fires early\nawait new Promise((r) => setTimeout(r, 500));'), []);
});

// #576 RV4-FE-FEB-2: only a `setTimeout(` CALL used to count, so each of these passed.
const RUNNER_TIMERS = {
  'an aliased runner-side setTimeout counts': ['const st = setTimeout;\nawait new Promise((r) => st(r, 500));', [1]],
  'a promisify(setTimeout) sleep counts': ["import { promisify } from 'node:util';\nconst sleep = promisify(setTimeout);\nawait sleep(500);", [2]],
  'a runner-side setInterval sleep counts': ['await new Promise((r) => {\n  const h = setInterval(() => { clearInterval(h); r(); }, 500);\n});', [2]],
};
for (const [name, [text, lines]] of Object.entries(RUNNER_TIMERS)) {
  test(name, () => assert.deepEqual(bad(text), lines));
}

test('a settle-annotated alias passes; an in-page setInterval and the test budget are not counted', () => {
  assert.deepEqual(bad('// settle: proves the debounce never fires early\nconst st = setTimeout;\nawait new Promise((r) => st(r, 500));'), []);
  assert.deepEqual(findSleeps('// settle: proves the debounce never fires early\nconst st = setTimeout;').map((s) => s.lineNo), [2], 'the alias is still a sleep, just an annotated one');
  assert.deepEqual(bad('await page.evaluate(() => new Promise((r) => {\n  const h = setInterval(() => { clearInterval(h); r(); }, 5);\n}));'), []);
  assert.deepEqual(bad('test.setTimeout(60000);\ntestInfo.setTimeout(30000);'), []);
  assert.deepEqual(bad("const label = 'setInterval';\nclearInterval(h);\nconst mySetTimeout = 1;"), []);
});

test('a setTimeout the browser runs (evaluate, addInitScript, waitForFunction) or in the mock is not a sleep', () => {
  const inPage = [
    'await page.evaluate(async () => {',
    "  const q = document.querySelector('x)');",
    '  await new Promise((r) => setTimeout(r, 100));',
    '});',
    'await page.addInitScript(() => { setTimeout(() => {}, 5); });',
    'await page.waitForFunction(() => new Promise((r) => setTimeout(() => r(true), 5)));',
    'testInfo.setTimeout(30000);',
  ].join('\n');
  assert.deepEqual(bad(inPage), []);
  // The same timer after the evaluate call has closed is the runner's again.
  assert.deepEqual(bad('await page.evaluate(() => 1);\nawait new Promise((r) => setTimeout(r, 5));'), [2]);
  const mock = 'export function install() { setTimeout(() => {}, 500); }';
  assert.deepEqual(scan(['e2e-playwright/mocks/ipc/x.ts'], () => mock).violations, []);
  assert.deepEqual(scan(['e2e-playwright/helpers/x.ts'], () => mock).violations.map((v) => v.lineNo), [1]);
});

// #576 NF-W9FE-4: sleeps in disguise the gate used to miss.
const DISGUISED = {
  'a wait whose failure is swallowed counts: locator.waitFor({ timeout }).catch(() => {})': [
    "await page.getByTestId('menu').waitFor({ state: 'visible', timeout: 500 }).catch(() => {});",
    [1],
  ],
  'every Playwright wait counts when swallowed (waitForSelector, waitForFunction, waitForEvent, waitForURL, waitForLoadState)': [
    [
      "await page.waitForSelector('#a', { timeout: 500 }).catch(() => undefined);",
      'await page.waitForFunction(() => window.ready, null, { timeout: 500 }).catch(() => null);',
      "await page.waitForEvent('download', { timeout: 500 }).catch(noop);",
      "await page.waitForURL('**/x', { timeout: 500 }).catch((e) => console.log(e));",
      "await page.waitForLoadState('networkidle').catch(() => { /* fine */ });",
    ].join('\n'),
    [1, 2, 3, 4, 5],
  ],
  'a swallowed web-first assertion, expect.poll or toPass counts': [
    [
      "await expect(page.getByTestId('toast')).toBeVisible({ timeout: 500 }).catch(() => {});",
      'await expect.poll(() => count(), { timeout: 500 }).toBe(2).catch(() => {});',
      'await expect(async () => { await check(); }).toPass({ timeout: 500 }).catch(() => {});',
    ].join('\n'),
    [1, 2, 3],
  ],
  'a swallowed wait split across lines counts at the line its statement starts': [
    "const x = 1;\nawait page\n  .locator('#a')\n  .waitFor({ state: 'hidden', timeout: 500 })\n  .catch(() => {});",
    [2],
  ],
  'a .then(() => {}) before the .catch does not hide it': [
    'await loc.waitFor({ timeout: 500 }).then(() => {}).catch(() => {});',
    [1],
  ],
  'a boolean probe whose value is thrown away counts': [
    'await loc.waitFor({ timeout: 500 }).then(() => true).catch(() => false);\nif (x) {\n  expect.poll(read).toBe(1).then(() => true).catch(() => false);\n}',
    [1, 3],
  ],
  'a wait swallowed by try / catch counts, at the wait': [
    "try {\n  await page.getByTestId('x').waitFor({ timeout: 500 });\n} catch {\n  // fine either way\n}\ntry { await expect(loc).toBeHidden({ timeout: 500 }); } catch (e) { log(e); }",
    [2, 6],
  ],
  "a concatenated member name counts: page['wait' + 'ForTimeout'](500)": ["await page['wait' + 'ForTimeout'](500);", [1]],
  'a concatenation across parts, quote styles, lines and a constant template counts': [
    "const nap = 'wait' + \"For\" +\n  `Timeout`;\nawait page[nap](500);\nawait page[`wait${'For'}Timeout`](500);",
    [1, 4],
  ],
  "a lone 'waitForTimeout' string used as a key counts": ["const m = 'waitForTimeout';\nawait page[m](500);", [1]],
};
for (const [name, [text, lines]] of Object.entries(DISGUISED)) {
  test(name, () => assert.deepEqual(bad(text), lines));
}

test('a settle annotation clears a swallowed wait and a concatenated name', () => {
  assert.deepEqual(bad("// settle: proves no toast appears; absence holds on the first read\nawait page.getByTestId('toast').waitFor({ timeout: 500 }).catch(() => {});"), []);
  assert.deepEqual(bad("// settle: proves the debounce never fires early\nawait page['wait' + 'ForTimeout'](500);"), []);
});

test('NOT a sleep: a wait that can fail, a rethrowing catch, a boolean probe, a swallowed action, prose and strings', () => {
  assert.deepEqual(bad("await page.getByTestId('x').waitFor({ timeout: 500 });"), []);
  assert.deepEqual(bad("await expect(loc).toBeVisible().catch((cause) => {\n  throw new Error('margin never settled', { cause });\n});"), []);
  assert.deepEqual(bad("try {\n  await loc.waitFor({ timeout: 500 });\n} catch (e) {\n  throw new Error('x', { cause: e });\n}"), []);
  assert.deepEqual(bad("try {\n  await loc.waitFor();\n} finally {\n  await page.close();\n}"), []);
  // A catch that asserts instead does not swallow the failure.
  assert.deepEqual(bad("try {\n  await openMenu(page);\n  await expect(canvas).toBeVisible();\n} catch {\n  await expect(canvas).toBeVisible();\n}"), []);
  assert.deepEqual(bad('await loc.waitFor({ timeout: 500 }).catch(async () => {\n  await expect(other).toBeVisible();\n});'), []);
  // A probe keeps the outcome as a value the test branches or asserts on.
  assert.deepEqual(bad('const ok = await loc.waitFor({ timeout: 500 }).then(() => true).catch(() => false);'), []);
  assert.deepEqual(bad('return expect.poll(read, { timeout: 500 }).toBe(true).then(() => true).catch(() => false);'), []);
  // An action, not a wait; and catch on things that are not waits.
  assert.deepEqual(bad('await button.click({ force: true, timeout: 1000 }).catch(() => {});'), []);
  assert.deepEqual(bad("await fetchThing().catch(() => {});\nawait page.goto('/').catch(() => {});"), []);
  assert.deepEqual(bad("// await loc.waitFor({ timeout: 5 }).catch(() => {});\nconst s = \"page['wait' + 'ForTimeout']\";"), []);
  assert.deepEqual(bad("const a = 'wait' + 'ForIt';\nconst b = 'waitForTimeout(100) is banned';\ntest('waitForTimeout is gone', () => {});"), []);
  // A try block with no wait, or a sync expect, is not a swallowed wait.
  assert.deepEqual(bad('try {\n  JSON.parse(raw);\n} catch {\n  // not JSON\n}\ntry { expect(a).toBe(1); } catch { }'), []);
});

// #576 RV9b-5: a type position or an object key named setTimeout is not a timer.
test('typeof setTimeout and a setTimeout object key are not sleeps; a value use still is', () => {
  assert.deepEqual(bad('let handle: ReturnType<typeof setTimeout> | null = null;'), []);
  assert.deepEqual(bad('const cfg = { setTimeout: false, retries: 2 };\nmock({\n  setInterval: 0,\n});'), []);
  assert.deepEqual(bad('const { setTimeout: nap } = globalThis;'), [1], 'a destructuring alias is the timer');
  assert.deepEqual(bad('const t = fast ? setTimeout : queueMicrotask;'), [1], 'a ternary operand is the timer');
  assert.deepEqual(bad('const timers = { setTimeout };\nconst more = { nap: setTimeout };'), [1, 2], 'a shorthand or a value is the timer');
});

// #576 RV10b-G2: a handler that rethrows everything EXCEPT the timeout swallows
// the wait's failure (Playwright's optional-wait idiom), as does one that only
// asserts something trivially true.
const CONDITIONAL_RETHROW = {
  'a try/catch that rethrows all but TimeoutError counts': [
    'try {\n  await loc.waitFor({ timeout: 1000 });\n} catch (e) {\n  if (!(e instanceof errors.TimeoutError)) throw e;\n}',
    [2],
  ],
  "a .catch that rethrows all but a 'Timeout' message counts": [
    "await loc.waitFor({ timeout: 1000 }).catch((e) => {\n  if (!String(e).includes('Timeout')) throw e;\n});",
    [1],
  ],
  'a catch that returns on the timeout before its throw counts': [
    'try {\n  await loc.waitFor({ timeout: 1000 });\n} catch (e) {\n  if (e instanceof errors.TimeoutError) return;\n  throw e;\n}',
    [2],
  ],
  'a throw in an if block or a ternary branch counts': [
    "await loc.waitFor({ timeout: 1000 }).catch((e) => {\n  if (e.name !== 'TimeoutError') {\n    throw e;\n  }\n});\nawait loc.waitFor({ timeout: 1 }).catch((e) => (isTimeout(e) ? null : fail(e)));",
    [1, 6],
  ],
  'a handler that asserts only a literal counts': ['await loc.waitFor({ timeout: 1000 }).catch(() => expect(true).toBe(true));', [1]],
};
for (const [name, [text, lines]] of Object.entries(CONDITIONAL_RETHROW)) {
  test(name, () => assert.deepEqual(bad(text), lines));
}

test('a settle annotation directly above the wait clears the optional-wait idiom', () => {
  assert.deepEqual(
    bad('try {\n  // settle: the hint toast may or may not appear; either is fine here\n  await loc.waitFor({ timeout: 1000 });\n} catch (e) {\n  if (!(e instanceof errors.TimeoutError)) throw e;\n}'),
    [],
  );
});

test('NOT a sleep: an unconditional rethrow after logging, on its own line, or after an if block', () => {
  assert.deepEqual(bad('try {\n  await loc.waitFor({ timeout: 500 });\n} catch (e) {\n  console.log(e)\n  throw e\n}'), []);
  assert.deepEqual(bad("await loc.waitFor().catch((e) => {\n  if (verbose) { log(e); }\n  throw new Error('menu never opened', { cause: e });\n});"), []);
  assert.deepEqual(bad('await loc.waitFor().catch((e) => expect(e).toBeUndefined());'), []);
});

// #576 RV10b-G3: swallowed-wait shapes the chain reader did not see.
const UNSEEN_SWALLOWS = {
  'a two-argument .then swallows the wait': ['await loc.waitFor({ timeout: 500 }).then(() => {}, () => {});', [1]],
  'Promise.allSettled of waits, its outcome thrown away': [
    "await Promise.allSettled([loc.waitFor({ timeout: 500 }), page.waitForURL('**/x')]);",
    [1],
  ],
  'Promise.race of waits with a swallowing .catch': [
    "await Promise.race([a.waitFor({ timeout: 500 }), b.waitFor({ timeout: 500 })]).catch(() => {});",
    [1],
  ],
  'an IIFE that waits, with a swallowing .catch': [
    'await (async () => {\n  await a.waitFor({ timeout: 500 });\n})().catch(() => {});',
    [1],
  ],
  'an optional-call .catch?.()': ['await loc.waitFor({ timeout: 500 })?.catch?.(() => {});', [1]],
};
for (const [name, [text, lines]] of Object.entries(UNSEEN_SWALLOWS)) {
  test(name, () => assert.deepEqual(bad(text), lines));
}

test('NOT a sleep: a two-argument .then that rethrows, a kept boolean or allSettled probe, race / all / IIFE without a swallowing catch', () => {
  assert.deepEqual(bad('await loc.waitFor().then(() => {}, (e) => { throw e; });'), []);
  assert.deepEqual(bad('const shown = await loc.waitFor({ timeout: 500 }).then(() => true, () => false);'), []);
  assert.deepEqual(bad("const [r] = await Promise.allSettled([loc.waitFor({ timeout: 500 })]);\nexpect(r.status).toBe('rejected');"), []);
  assert.deepEqual(bad("await Promise.race([a.waitFor(), b.waitFor()]);\nawait Promise.all([page.waitForEvent('download'), btn.click()]);"), []);
  assert.deepEqual(bad("await Promise.allSettled([fetchA(), fetchB()]);\nawait (async () => { await btn.click(); })().catch(() => {});"), []);
  assert.deepEqual(bad("await Promise.race([a.waitFor(), b.waitFor()]).catch((e) => { throw new Error('neither', { cause: e }); });"), []);
});

// #576 RV10b-G4: a destructure binds the timer whatever follows the pattern -
// a parameter list, a for-of, a type annotation - not only `= value`.
test('a timer bound through a parameter, for-of or typed destructure counts', () => {
  assert.deepEqual(bad('(({ setTimeout: nap }) => nap(done, 100))(globalThis);'), [1]);
  assert.deepEqual(bad('for (const { setTimeout: nap } of [globalThis]) nap(done, 100);'), [1]);
  assert.deepEqual(bad('const { setTimeout: nap }: typeof globalThis = globalThis;'), [1]);
  assert.deepEqual(bad('const { timers: { setTimeout: nap } } = runtime;'), [1]);
  assert.deepEqual(bad('async function f({ setTimeout: nap }: typeof globalThis) {\n  await new Promise((r) => nap(r, 5));\n}'), [1]);
});

test('NOT a sleep: an object key in a call argument, an arrow-returned object, or a type', () => {
  assert.deepEqual(bad('mock({ setTimeout: 0 });\nconst make = () => ({ setInterval: 1 });\ntype T = { a: { setTimeout: number } };'), []);
});

test('a regex literal holding /* does not hide a later sleep (shared stripper)', () => {
  assert.deepEqual(bad('const LEAD = /^\\/*/;\nawait page.waitForTimeout(5);\n/* end */'), [2]);
});

test('scan counts annotated settles separately from violations', () => {
  const r = scan(['a.ts', 'b.ts'], (rel) => (rel === 'a.ts' ? '// settle: proves nothing moved here\nawait page.waitForTimeout(1);' : 'await page.waitForTimeout(1);'));
  assert.equal(r.settles, 1);
  assert.deepEqual(r.violations, [{ rel: 'b.ts', lineNo: 1 }]);
});

test('findSleeps reports the reason it accepted', () => {
  assert.deepEqual(findSleeps('// settle: proves the menu never opens\nawait page.waitForTimeout(50);'), [{ lineNo: 2, reason: 'proves the menu never opens' }]);
});

test('main fails without e2e-playwright/ and on a symlinked spec; passes a clean tree', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'no-sleep-'));
  const quiet = () => {};
  try {
    // Repo-relative, never the absolute root: it names the checkout's location (#576 W51).
    const missing = [];
    assert.equal(main(root, quiet, (m) => missing.push(m)), 1);
    assert.deepEqual(missing, ['check-e2e-no-sleep: e2e-playwright/ not found (run from the repo root).']);
    fs.mkdirSync(path.join(root, 'e2e-playwright'));
    fs.writeFileSync(path.join(root, 'e2e-playwright', 'a.spec.ts'), "await expect(page.getByTestId('x')).toBeVisible();\n");
    assert.equal(main(root, quiet, quiet), 0);
    fs.writeFileSync(path.join(root, 'outside.ts'), 'await page.waitForTimeout(100);\n');
    fs.symlinkSync('../outside.ts', path.join(root, 'e2e-playwright', 'b.spec.ts'));
    const lines = [];
    assert.equal(main(root, quiet, (m) => lines.push(m)), 1);
    assert.ok(lines.some((l) => l.includes('b.spec.ts  is a symlink')), lines.join('\n'));
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('the live tree passes', () => {
  assert.equal(main(undefined, () => {}, () => {}), 0);
});

if (failures.length > 0) {
  console.error(`✗ check-e2e-no-sleep.test: ${failures.length} failure(s):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ check-e2e-no-sleep.test: ${passed} checks passed.`);
