// Critical-feature canary gate for the @smoke set (#576 E5).
//
// The @smoke Playwright subset is the only part of the browser tier that gates
// a merge (the full suite runs nightly). Its composition used to be an
// accident: whoever last tagged a test decided what was covered, so it drifted
// to a third gas-planning while project deletion, the detail panel and the
// move tools had no merge-time coverage at all. docs/TESTING.md now commits a
// list of critical user paths, each with ONE named canary test; this gate
// fails when a listed canary is no longer an @smoke test - renamed, untagged,
// skipped or deleted - so the list cannot decay the way the tags did.
//
// THE LIST - a markdown table between the `smoke-features:start` /
// `smoke-features:end` markers in docs/TESTING.md, one row per feature:
//
//   | Feature | Spec | Canary (title contains) |
//   |---|---|---|
//   | Delete a project | `globalview-delete-project.spec.ts` | `confirming deletes` |
//
// A row passes when that spec has a NON-skipped test whose title contains both
// `@smoke` and the canary fragment (skipped = declared skip/fixme, inside a
// describe.skip/fixme group, or under a skip/fixme annotation; see testTitles).
// The TestInfo forms (`testInfo.skip()`, `test.info().fixme(cond)`) count as
// annotations, and a `test.fail` test - declared or annotated - is no canary,
// since it passes only while the feature is broken (#576 RV4-FE-FEB-1).
// Comments are stripped first, so a spec
// that merely mentions @smoke in prose (custom-renderer-health.spec.ts does)
// does not count. Only the test title is read - the repo's convention is the
// title suffix, not Playwright's `tag` option (docs/TESTING.md).
//
// WHERE IT RUNS - the smoke job (e2e-smoke.yml), before the browser install,
// and the leak-scan workflow: the list lives in docs/, which `smoke`
// path-ignores, so a docs-only edit to the table would otherwise run no gate
// that reads it (the "gate consumes a path-ignored file" class).
//
// Usage:  node scripts/check-smoke-features.mjs   (or: pnpm lint:smoke-features)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { matchBrace, matchParen, stripComments } from './lib/js-comments.mjs';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const LIST_FILE = 'docs/TESTING.md';
export const SPEC_DIR = 'e2e-playwright';
const START = '<!-- smoke-features:start -->';
const END = '<!-- smoke-features:end -->';

// Parse the canary table. Returns {rows: [{feature, spec, canary, line}], error}.
export function parseFeatureList(md) {
  const lines = md.split(/\r?\n/);
  const a = lines.findIndex((l) => l.trim() === START);
  const b = lines.findIndex((l, i) => i > a && l.trim() === END);
  if (a < 0 || b < 0) return { rows: [], error: `${LIST_FILE} has no ${START} ... ${END} block` };
  const rows = [];
  const problems = [];
  for (let i = a + 1; i < b; i++) {
    const t = lines[i].trim();
    if (!t.startsWith('|')) continue;
    const cells = t.replace(/^\||\|$/g, '').split('|').map((c) => c.trim());
    if (cells.every((c) => /^:?-{3,}:?$/.test(c))) continue; // separator row
    if (/^feature$/i.test(cells[0])) continue; // header row
    const unquote = (c) => c.replace(/^`(.*)`$/, '$1').replace(/^"(.*)"$/, '$1').trim();
    const [feature, spec, canary] = [cells[0] ?? '', unquote(cells[1] ?? ''), unquote(cells[2] ?? '')];
    if (cells.length !== 3 || !feature || !/^[\w.-]+\.spec\.ts$/.test(spec) || !canary) {
      problems.push(`${LIST_FILE}:${i + 1}: expected | Feature | \`<file>.spec.ts\` | \`<title fragment>\` |`);
      continue;
    }
    rows.push({ feature, spec, canary, line: i + 1 });
  }
  if (problems.length) return { rows, error: problems.join('\n  ') };
  if (rows.length === 0) return { rows, error: `the ${START} table in ${LIST_FILE} has no feature rows` };
  return { rows, error: null };
}

// Titles of the runnable (not skipped / fixme'd) tests in a spec's source.
// Handles a title on the line after `test(` and all three quote styles.
//
// A test does not run - so it is no canary - when (#576 RV3-FE-12):
//   - it is declared `test.skip('title', ...)` / `test.fixme('title', ...)`
//     (TEST_CALL_RE never matches those);
//   - it sits anywhere inside a `test.describe.skip(...)` /
//     `test.describe.fixme(...)` group;
//   - its body, an enclosing describe, or the file's top level calls the
//     annotation form `test.skip()` / `test.skip(condition, reason)` (or the
//     same for fixme). A condition is not evaluated: a canary must run on
//     every project, so any skip annotation in its scope disqualifies it;
//   - (#576 RV4-FE-FEB-1) the annotation is made through the TestInfo API -
//     `testInfo.skip()`, `info.fixme(cond)` (the parameter can have any
//     name), `test.info().skip(cond, reason)` - or it is a `fail` one: a
//     `test.fail('title', ...)` test, or `test.fail()` / `testInfo.fail()`
//     in scope, passes CI only while the feature is BROKEN, so it is no canary
//     (TEST_CALL_RE does not read `test.fail('title')` as a test at all).
// Scopes are the paren extents of the calls (scripts/lib/js-comments.mjs
// matchParen), so nesting depth and line breaks do not matter. The
// annotation's receiver is `test`, `test.info()` or a TestInfo in scope:
//   - the TestInfo parameter of a test or hook callback that encloses it - the
//     second parameter, after the fixtures object (`async ({ page }, testInfo)
//     =>`), either of them type-annotated or not (`({ page }: { page: Page },
//     testInfo: TestInfo)`, #576 RV10b-G1);
//   - a local bound to `test.info()` (`const info = test.info()`), within the
//     test or hook callback that declares it (#576 RV10b-G1);
// so `reader.skip(4)` or a helper's own `info.skip()` is not one (#576 RV9b-6);
// the lookbehind keeps `test.describe.skip(` / `test.step.skip(` (a member of a
// member) from reading as one. A test or hook whose callback is a NAMED
// function (`test.beforeEach(gate)`, `async function gate({ page }, testInfo)
// { testInfo.skip() }`, or a `const gate = async (...) =>`) is read through
// that function's own TestInfo parameter: a hook's skips the describe that
// holds the hook call, a test's skips that test (#576 RV10b-G1). A skip made in
// a helper the canary calls and hands its TestInfo to is not seen.
const TEST_CALL_RE = /\btest(?:\.(only|slow))?\s*\(\s*(['"`])((?:\\.|(?!\2)[^\\])*)\2/g;
const DECLARED_NOT_RUN_RE = /\btest\.(skip|fixme|fail)\s*\(\s*(['"`])((?:\\.|(?!\2)[^\\])*)\2/g;
const SKIPPED_GROUP_RE = /\b(?:test\.)?describe\.(skip|fixme)\s*\(/g;
const GROUP_RE = /\b(?:test\.)?describe(?:\.(?:only|parallel|serial))?\s*\(/g;
const ANNOTATION_RE = /(?<![\w$.])(test\.info\(\)|[A-Za-z_$][\w$]*)\.(skip|fixme|fail)\s*\(\s*(?!['"`])/g;
// A test or hook call (group 1: its modifier or hook name), and - read from the
// start of its arguments, past an optional title and details object - its
// callback's TestInfo parameter, or the name of the function it is handed.
const TEST_OR_HOOK_RE = /\btest(?:\.(only|slow|skip|fixme|fail|beforeEach|afterEach|beforeAll|afterAll))?\s*\(/g;
const HOOKS = new Set(['beforeEach', 'afterEach', 'beforeAll', 'afterAll']);
// `{ ... }` with one level of nesting: a destructure, a details object, an object type.
const BRACED = String.raw`\{(?:[^{}]|\{[^{}]*\})*\}`;
const TYPED = String.raw`(?:\s*:\s*(?:${BRACED}|[\w$.]+(?:<[^<>]*>)?))?`;
// From a parameter list's `(`: the fixtures destructure, then the TestInfo name.
const FIXTURES_THEN_INFO = String.raw`\(\s*${BRACED}${TYPED}\s*,\s*([A-Za-z_$][\w$]*)`;
const LEAD = String.raw`^\s*(?:(['"\`])(?:\\.|(?!\1)[^\\])*\1\s*,\s*)?(?:${BRACED}\s*,\s*)?`;
const TESTINFO_PARAM_RE = new RegExp(`${LEAD}(?:async\\s+)?(?:function\\s*[\\w$]*\\s*)?${FIXTURES_THEN_INFO}`);
const NAMED_CALLBACK_RE = new RegExp(`${LEAD}([A-Za-z_$][\\w$]*)\\s*$`);
const PARAMS_INFO_RE = new RegExp(`^${FIXTURES_THEN_INFO}`);
const INFO_LOCAL_RE = /\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::\s*[\w$.]+\s*)?=\s*test\.info\(\)/g;

// [{start, end, kind, open}] extents of every call `re` matches (its `(` is the
// match's last character); `kind` is the match's group 1, if any.
function callExtents(code, re) {
  const out = [];
  let m;
  re.lastIndex = 0;
  while ((m = re.exec(code)) !== null) {
    const open = code.lastIndexOf('(', m.index + m[0].length - 1);
    out.push({ start: m.index, end: matchParen(code, open), kind: m[1], open });
  }
  return out;
}

// Why an annotation of `kind` makes a test no canary (#576 RV10b-G5): a skip or
// fixme stops it running; a fail one runs, and passes only while it fails.
const noCanary = (kind, what) =>
  kind === 'fail'
    ? `it is no canary: ${what}, so it passes only while the feature is broken`
    : `it does not run: ${what}`;

// The innermost of `scopes` that encloses `at`, else the whole file.
const innermost = (scopes, at, code) =>
  scopes
    .filter((s) => s.start < at && at < s.end)
    .reduce((best, s) => (best === null || s.start > best.start ? s : best), null) ?? { start: 0, end: code.length };

// The first annotation the function NAMED `name` makes through its own TestInfo
// parameter - `function name(...)` or `const name = (async) (...) =>` /
// `function (...)` - as {kind, receiver}, or null (#576 RV10b-G1).
function namedFunctionAnnotation(code, name) {
  const id = name.replace(/\$/g, '\\$');
  const def = new RegExp(
    String.raw`(?:\bfunction\s+${id}\s*|\b(?:const|let|var)\s+${id}\s*(?::[^=;]+)?=\s*(?:async\s+)?(?:function\s*[\w$]*\s*)?)(?=\()`,
    'g',
  );
  let d;
  while ((d = def.exec(code)) !== null) {
    const open = d.index + d[0].length;
    const paramsEnd = matchParen(code, open);
    const param = PARAMS_INFO_RE.exec(code.slice(open, paramsEnd))?.[1];
    if (param === undefined) continue;
    const brace = /^\s*(?::[^={]+)?(?:=>\s*)?\{/.exec(code.slice(paramsEnd));
    const bodyEnd = brace
      ? matchBrace(code, paramsEnd + brace[0].length - 1)
      : (code.indexOf('\n', paramsEnd) + 1 || code.length);
    const id2 = param.replace(/\$/g, '\\$');
    const hit = new RegExp(String.raw`(?<![\w$.])${id2}\.(skip|fixme|fail)\s*\(\s*(?!['"\`])`).exec(
      code.slice(paramsEnd, bodyEnd),
    );
    if (hit) return { kind: hit[1], receiver: param };
  }
  return null;
}

// Every test the spec declares: {title, canary, why}. `canary` is false for a
// test that does not run (skipped / fixme'd) or runs as an expected failure,
// and `why` says which (#576 RV9b-6, RV10b-G5).
export function testEntries(source) {
  const code = stripComments(source.split('\n')).join('\n');
  const lineOf = (at) => code.slice(0, at).split('\n').length;
  const tests = [];
  let m;
  TEST_CALL_RE.lastIndex = 0;
  while ((m = TEST_CALL_RE.exec(code)) !== null) {
    const open = code.indexOf('(', m.index);
    tests.push({ title: m[3], start: m.index, end: matchParen(code, open) });
  }
  const groups = callExtents(code, GROUP_RE);
  const skipped = callExtents(code, SKIPPED_GROUP_RE).map((g) => ({
    ...g,
    why: noCanary('skip', `it is in a describe.${g.kind} group (line ${lineOf(g.start)})`),
  }));
  // The TestInfo receivers in scope: each test or hook callback's own parameter,
  // and a local bound to test.info() within the callback that declares it.
  const calls = callExtents(code, TEST_OR_HOOK_RE);
  const infoScopes = calls
    .map((c) => ({ ...c, name: TESTINFO_PARAM_RE.exec(code.slice(c.open + 1, c.end))?.[2] }))
    .filter((c) => c.name !== undefined);
  INFO_LOCAL_RE.lastIndex = 0;
  while ((m = INFO_LOCAL_RE.exec(code)) !== null) {
    infoScopes.push({ start: m.index, end: innermost(calls, m.index, code).end, name: m[1] });
  }
  // An annotation skips its innermost enclosing test or describe, else the file.
  const scopes = [...tests, ...groups];
  ANNOTATION_RE.lastIndex = 0;
  while ((m = ANNOTATION_RE.exec(code)) !== null) {
    const at = m.index;
    const receiver = m[1];
    const isTestInfo =
      receiver === 'test' ||
      receiver === 'test.info()' ||
      infoScopes.some((c) => c.name === receiver && c.start < at && at < c.end);
    if (!isTestInfo) continue;
    skipped.push({ ...innermost(scopes, at, code), why: noCanary(m[2], `${receiver}.${m[2]}() in its scope (line ${lineOf(at)})`) });
  }
  // A test or hook handed a named function: that function's own annotation
  // skips the test, or the describe holding the hook call.
  for (const c of calls) {
    const name = NAMED_CALLBACK_RE.exec(code.slice(c.open + 1, c.end - 1))?.[2];
    const hit = name === undefined ? null : namedFunctionAnnotation(code, name);
    if (hit === null) continue;
    const via = `${name}'s ${hit.receiver}.${hit.kind}() via test${c.kind ? `.${c.kind}` : ''}(${name}) (line ${lineOf(c.start)})`;
    const scope = HOOKS.has(c.kind) ? innermost(groups, c.start, code) : { start: c.start, end: c.end };
    skipped.push({ ...scope, why: noCanary(hit.kind, via) });
  }
  const entries = tests.map((t) => {
    const stop = skipped.find((s) => s.start <= t.start && t.start < s.end);
    return { title: t.title, canary: stop === undefined, why: stop?.why ?? null };
  });
  DECLARED_NOT_RUN_RE.lastIndex = 0;
  while ((m = DECLARED_NOT_RUN_RE.exec(code)) !== null) {
    entries.push({ title: m[3], canary: false, why: noCanary(m[1], `it is declared test.${m[1]}(...) (line ${lineOf(m.index)})`) });
  }
  return entries;
}

// Titles of the tests that can be canaries: they run, and not as expected failures.
export function testTitles(source) {
  return testEntries(source)
    .filter((e) => e.canary)
    .map((e) => e.title);
}

// `read(rel)` returns file text or throws. Returns {problems, covered}.
export function checkFeatures(read) {
  let md;
  try {
    md = read(LIST_FILE);
  } catch (e) {
    return { problems: [`${LIST_FILE} unreadable: ${e.code || e.message}`], covered: [] };
  }
  const { rows, error } = parseFeatureList(md);
  if (error) return { problems: [error], covered: [] };
  const problems = [];
  const covered = [];
  const cache = new Map();
  for (const row of rows) {
    const rel = `${SPEC_DIR}/${row.spec}`;
    if (!cache.has(rel)) {
      try {
        cache.set(rel, testEntries(read(rel)));
      } catch {
        cache.set(rel, null);
      }
    }
    const entries = cache.get(rel);
    if (entries === null) {
      problems.push(`"${row.feature}": ${rel} does not exist (${LIST_FILE}:${row.line})`);
      continue;
    }
    const titles = entries.filter((e) => e.canary).map((e) => e.title);
    const hit = titles.find((t) => t.includes('@smoke') && t.includes(row.canary));
    if (!hit) {
      const near = titles.find((t) => t.includes(row.canary));
      const stopped = entries.find((e) => !e.canary && e.title.includes(row.canary));
      const why = near
        ? ` (found it untagged: "${near}")`
        : stopped
          ? ` (found it, but ${stopped.why})`
          : ' (renamed or deleted?)';
      problems.push(`"${row.feature}": no @smoke test in ${rel} whose title contains "${row.canary}"${why} - ${LIST_FILE}:${row.line}`);
      continue;
    }
    covered.push({ feature: row.feature, rel, title: hit });
  }
  return { problems, covered };
}

export function main(read = (rel) => fs.readFileSync(path.join(REPO_ROOT, rel), 'utf8'), log = console.log, err = console.error) {
  const { problems, covered } = checkFeatures(read);
  if (problems.length === 0) {
    log(`✓ smoke features: all ${covered.length} critical features in ${LIST_FILE} have an @smoke canary.`);
    return 0;
  }
  err(`\n✗ smoke-feature check failed - ${problems.length} problem(s):\n`);
  for (const p of problems) err(`  ${p}`);
  err(
    `\nEvery critical feature in ${LIST_FILE} ("Critical features") needs its canary to stay an\n` +
      '@smoke test. Re-tag it, point the row at the test that replaced it, or - if the feature\n' +
      'is no longer critical - remove the row in the same PR, with a reason.\n',
  );
  return 1;
}

const isMainModule =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMainModule) process.exit(main());
