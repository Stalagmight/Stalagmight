// Fixture tests for the critical-feature canary gate (scripts/check-smoke-features.mjs).
//
// Pins the table format, what counts as an @smoke canary (a non-skipped test
// title, not a comment mention - the custom-renderer-health trap), and every
// fail-closed branch. Pure Node, no test framework; the reader is injected, and
// the last case runs the gate on the live tree.
// Run: node scripts/check-smoke-features.test.mjs  (also a CI smoke + leak-scan step).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkFeatures, parseFeatureList, testTitles } from './check-smoke-features.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const readLive = (rel) => fs.readFileSync(path.join(ROOT, rel), 'utf8');

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

const doc = (rows) => [
  '# Testing', '', '<!-- smoke-features:start -->',
  '| Feature | Spec | Canary (title contains) |', '|---|---|---|',
  ...rows, '<!-- smoke-features:end -->', '',
].join('\n');
const run = (files) => checkFeatures((rel) => {
  if (!(rel in files)) throw new Error(`ENOENT ${rel}`);
  return files[rel];
});

const ROW = '| Delete a project | `delete.spec.ts` | `confirming deletes` |';

test('a canary that is an @smoke test passes', () => {
  const r = run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': "test('opens a dialog, and confirming deletes @smoke', async () => {});" });
  assert.deepEqual(r.problems, []);
  assert.equal(r.covered.length, 1);
});

test('an untagged canary fails and says it was found untagged', () => {
  const r = run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': "test('opens a dialog, and confirming deletes', async () => {});" });
  assert.equal(r.problems.length, 1);
  assert.match(r.problems[0], /no @smoke test .* "confirming deletes" \(found it untagged/);
});

test('a renamed canary fails; another @smoke test in the same file does not satisfy the row', () => {
  const r = run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': "test('something else @smoke', async () => {});" });
  assert.match(r.problems[0], /renamed or deleted/);
});

test('a skipped or fixme canary does not count', () => {
  for (const kind of ['skip', 'fixme']) {
    const r = run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': `test.${kind}('confirming deletes @smoke', async () => {});` });
    assert.equal(r.problems.length, 1, kind);
  }
  const only = run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': "test.only('confirming deletes @smoke', async () => {});" });
  assert.deepEqual(only.problems, []);
});

// #576 RV3-FE-12: each of these used to pass the gate with a canary that never runs.
test('a canary inside a describe.skip / describe.fixme group does not count, at any depth', () => {
  for (const kind of ['skip', 'fixme']) {
    const spec = `test.describe.${kind}('Group', () => {\n  test.describe('inner', () => {\n    test('confirming deletes @smoke', async () => {});\n  });\n});`;
    assert.equal(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec }).problems.length, 1, kind);
  }
});

test('a skip/fixme annotation in the canary body, its describe, or the file top level disqualifies it', () => {
  const cases = {
    body: "test('confirming deletes @smoke', async () => {\n  test.skip(true, 'broken');\n});",
    bodyNoArgs: "test('confirming deletes @smoke', async () => {\n  test.fixme();\n});",
    describe: "test.describe('Group', () => {\n  test.skip(({ browserName }) => browserName === 'chromium', 'no');\n  test('confirming deletes @smoke', async () => {});\n});",
    file: "test.skip(({ browserName }) => browserName === 'chromium', 'no');\ntest('confirming deletes @smoke', async () => {});",
  };
  for (const [name, spec] of Object.entries(cases)) {
    assert.equal(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec }).problems.length, 1, name);
  }
});

test('a skip outside the canary scope (a sibling test, another group, a skipped group that closed) does not', () => {
  const spec = [
    "test('other', async () => { test.skip(true, 'x'); });",
    "test.describe('A', () => { test.skip(); test('a', async () => {}); });",
    "test.describe.skip('B', () => { test('b @smoke', async () => {}); });",
    "test.describe('C', () => {",
    "  test('confirming deletes @smoke', async () => { await expect(x).toHaveText(/\\(/); });",
    '});',
  ].join('\n');
  const r = run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec });
  assert.deepEqual(r.problems, []);
  assert.deepEqual(testTitles(spec), ['confirming deletes @smoke']);
});

// #576 RV4-FE-FEB-1: each of these used to pass the gate with a canary that never runs,
// or one that passes only while the feature is broken.
const DISQUALIFIED = {
  'a TestInfo skip disqualifies the canary: testInfo.skip() in the body': "test('confirming deletes @smoke', async ({ page }, testInfo) => {\n  testInfo.skip();\n});",
  'a TestInfo fixme disqualifies the canary: info.fixme() with a renamed parameter': "test('confirming deletes @smoke', async ({ page }, info) => {\n  info.fixme(true, 'broken');\n});",
  'a TestInfo skip disqualifies the canary: test.info().skip(cond, why)': "test('confirming deletes @smoke', async ({ browserName }) => {\n  test.info().skip(browserName === 'webkit', 'why');\n});",
  'a TestInfo skip disqualifies the canary: testInfo.skip() in the describe beforeEach': "test.describe('G', () => {\n  test.beforeEach(async ({}, testInfo) => { testInfo.skip(); });\n  test('confirming deletes @smoke', async () => {});\n});",
  "a test.fail canary does not count: declared test.fail('confirming deletes @smoke', ...)": "test.fail('confirming deletes @smoke', async () => {});",
  'a test.fail canary does not count: test.fail() in its body': "test('confirming deletes @smoke', async () => {\n  test.fail();\n});",
};
for (const [name, spec] of Object.entries(DISQUALIFIED)) {
  test(name, () => {
    assert.equal(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec }).problems.length, 1);
  });
}

test('a TestInfo skip in a sibling, a closed describe.skip group and a titled test.step.skip in the canary do not disqualify it', () => {
  const spec = [
    "test('other', async ({}, testInfo) => { testInfo.skip(); });",
    "test.describe.skip('B', () => { test('b @smoke', async () => {}); });",
    "test('confirming deletes @smoke', async () => {",
    "  await test.step.skip('step title', async () => {});",
    '});',
  ].join('\n');
  assert.deepEqual(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec }).problems, []);
  assert.deepEqual(testTitles(spec), ['confirming deletes @smoke']);
  const only = run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': "test.only('confirming deletes @smoke', async () => {});" });
  assert.deepEqual(only.problems, [], 'test.only is still a canary');
});

// #576 RV9b-6: only an annotation through `test`, `test.info()` or a test / hook
// callback's own TestInfo parameter is one; and a canary that exists but does not
// run says so instead of "renamed or deleted".
test('a skip/fail call on some other object, or in a helper on an untyped parameter, does not disqualify the canary', () => {
  const domain = "test('confirming deletes @smoke', async () => {\n  reader.skip(4);\n  queue.fail(new Error('x'));\n});";
  assert.deepEqual(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': domain }).problems, []);
  const helper = [
    'function needsWebgl(info) {',
    "  info.skip(!webgl, 'no webgl');",
    '}',
    "test('other', async ({}, testInfo) => { needsWebgl(testInfo); });",
    "test('confirming deletes @smoke', async () => {});",
  ].join('\n');
  assert.deepEqual(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': helper }).problems, []);
  // The canary's own TestInfo, under any name, still disqualifies it; a sibling's does not.
  const own = "test('other', async ({}, info) => {});\ntest('confirming deletes @smoke', async ({ page }, ti) => {\n  ti.skip();\n});";
  assert.equal(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': own }).problems.length, 1);
  const tagged = "test('confirming deletes @smoke', { tag: '@x' }, async ({ page }, ti) => {\n  ti.fixme();\n});";
  assert.equal(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': tagged }).problems.length, 1);
});

// #576 RV10b-G1: three skipped-canary shapes the RV9b-6 receiver narrowing let
// through - a TestInfo held in a local, a typed callback, a named hook function.
const G1_DISQUALIFIED = {
  'a TestInfo held in a local disqualifies the canary: const info = test.info(); info.skip()':
    "test('confirming deletes @smoke', async () => {\n  const info = test.info();\n  info.skip(true, 'broken');\n});",
  'a typed callback still names its TestInfo: ({ page }: { page: Page }, testInfo: TestInfo)':
    "test('confirming deletes @smoke', async ({ page }: { page: Page }, testInfo: TestInfo) => {\n  testInfo.skip(true, 'broken');\n});",
  'a typed callback still names its TestInfo: ({ page }: Fixtures, info)':
    "test('confirming deletes @smoke', async ({ page }: PlaywrightTestArgs, info) => {\n  info.fixme();\n});",
  'a named hook function skips through its own TestInfo: test.beforeEach(gate)':
    "async function gate({ page }, testInfo) {\n  testInfo.skip(true, 'broken');\n}\ntest.beforeEach(gate);\ntest('confirming deletes @smoke', async () => {});",
  'a named hook arrow skips through its own TestInfo, in the canary describe':
    "const gate = async ({}: object, info: TestInfo) => { info.fixme(); };\ntest.describe('G', () => {\n  test.beforeEach(gate);\n  test('confirming deletes @smoke', async () => {});\n});",
};
for (const [name, spec] of Object.entries(G1_DISQUALIFIED)) {
  test(name, () => {
    assert.equal(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec }).problems.length, 1);
  });
}

test('a local that is not test.info(), a named hook in another describe and a named function no hook takes do not disqualify the canary', () => {
  const spec = [
    "async function gate({ page }, testInfo) { testInfo.skip(true, 'x'); }",
    "async function unused({ page }, testInfo) { testInfo.skip(true, 'x'); }",
    "test.describe('Other', () => { test.beforeEach(gate); test('a', async () => {}); });",
    "test.describe('C', () => {",
    "  test.beforeEach(async ({ page }: { page: Page }) => { await page.goto('/'); });",
    "  test('confirming deletes @smoke', async ({ page }: { page: Page }, testInfo: TestInfo) => {",
    '    const info = makeReader();',
    '    info.skip(2);',
    "    testInfo.attach('x', { body: 'y' });",
    '  });',
    '});',
  ].join('\n');
  assert.deepEqual(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec }).problems, []);
});

// #576 RV10b-G5: a test.fail test runs - it is no canary because it passes only
// while the feature is broken, and the message says that.
test('a test.fail canary is reported as no canary, not as one that does not run', () => {
  for (const spec of [
    "test.fail('confirming deletes @smoke', async () => {});",
    "test('confirming deletes @smoke', async () => {\n  test.fail();\n});",
    "test('confirming deletes @smoke', async ({}, testInfo) => {\n  testInfo.fail();\n});",
  ]) {
    const [problem] = run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec }).problems;
    assert.match(problem ?? '', /found it, but it is no canary: .*passes only while the feature is broken/, spec);
    assert.doesNotMatch(problem ?? '', /does not run/, spec);
  }
});

test('a canary that is there but does not run says why, not "renamed or deleted"', () => {
  for (const spec of [
    "test.skip('confirming deletes @smoke', async () => {});",
    "test.describe.fixme('G', () => {\n  test('confirming deletes @smoke', async () => {});\n});",
    "test('confirming deletes @smoke', async ({}, testInfo) => {\n  testInfo.skip();\n});",
  ]) {
    const [problem] = run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec }).problems;
    assert.match(problem ?? '', /found it, but it does not run/, spec);
    assert.doesNotMatch(problem ?? '', /renamed or deleted/, spec);
  }
});

test('an @smoke mention in a comment is not a tag (the custom-renderer-health trap)', () => {
  const spec = ['/**', ' * Kept @smoke so a regression trips the merge gate - confirming deletes.', ' */', "test('confirming deletes', async () => {});"].join('\n');
  assert.equal(run({ 'docs/TESTING.md': doc([ROW]), 'e2e-playwright/delete.spec.ts': spec }).problems.length, 1);
});

test('titles split across lines and in every quote style are read', () => {
  assert.deepEqual(testTitles("test(\n  'a @smoke',\n  async () => {});"), ['a @smoke']);
  assert.deepEqual(testTitles('test("b @smoke", f); test(`c ${x} @smoke`, f);'), ['b @smoke', 'c ${x} @smoke']);
  assert.deepEqual(testTitles("test.describe('group', () => {}); test.skip(cond, 'why');"), []);
});

test('a missing spec file, a malformed row, a missing block or an empty table each fail', () => {
  assert.match(run({ 'docs/TESTING.md': doc([ROW]) }).problems[0], /does not exist/);
  assert.match(run({ 'docs/TESTING.md': doc(['| Delete | delete.spec.ts |']) }).problems[0], /expected \| Feature/);
  assert.match(run({ 'docs/TESTING.md': '# no table here\n' }).problems[0], /has no <!-- smoke-features:start -->/);
  assert.match(run({ 'docs/TESTING.md': doc([]) }).problems[0], /no feature rows/);
  assert.match(run({}).problems[0], /unreadable/);
});

test('parseFeatureList strips backticks and skips the header and separator rows', () => {
  const { rows, error } = parseFeatureList(doc([ROW]));
  assert.equal(error, null);
  assert.deepEqual(rows.map((r) => [r.feature, r.spec, r.canary]), [['Delete a project', 'delete.spec.ts', 'confirming deletes']]);
});

test('the live list passes: every critical feature in docs/TESTING.md has its @smoke canary', () => {
  const r = checkFeatures((rel) => readLive(rel));
  assert.deepEqual(r.problems, []);
  assert.ok(r.covered.length >= 15, `only ${r.covered.length} features listed`);
});

if (failures.length > 0) {
  console.error(`✗ check-smoke-features.test: ${failures.length} failure(s):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ check-smoke-features.test: ${passed} checks passed.`);
