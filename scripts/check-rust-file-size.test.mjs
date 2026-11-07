// Fixture tests for the Rust file-size ratchet (scripts/check-rust-file-size.mjs).
//
// Pins the prod-LOC strip heuristic against the exact inputs that the naive
// brace-counter got wrong: a braceless `#[cfg(test)]` item that swallowed the
// next production fn, and an unbalanced brace inside a test string that ran the
// strip to EOF. Both are silent UNDER-counts - the dangerous direction for a
// ratchet - so they get first-class fixtures here.
//
// Pure Node, no test framework (same style as the other scripts/*.test.mjs
// style). Run: node scripts/check-rust-file-size.test.mjs  (also a CI `check` step).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { spawnSync } from 'node:child_process';
import {
  prodLoc, classify, markCfgTestItems, maskNonCode, discoverSrcRoots, discoverExtraRoots, WARN_LOC, FAIL_LOC,
  TEST_WARN_LOC, TEST_FAIL_LOC, TEST_LIMITS, TEST_FILE_RE, declaredUnderTestCfg, isTestModule, testLoc,
  tightenRows, unrecordedFiles,
} from './check-rust-file-size.mjs';

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

// --- prodLoc: #[cfg(test)] stripping -----------------------------------------

test('strips a trailing #[cfg(test)] mod block, keeps production', () => {
  const src = ['fn a() {', '    work();', '}', '', '#[cfg(test)]', 'mod tests {', '    fn t() {', '        a();', '    }', '}'].join('\n');
  // production: `fn a() {`, `work();`, `}` = 3
  assert.equal(prodLoc(src), 3);
});

test('REGRESSION: braceless `#[cfg(test)] use …;` does NOT swallow the next fn', () => {
  // The naive scanner hunted forward to the first `{` and stripped `production`.
  const src = ['#[cfg(test)]', 'use crate::foo::bar;', 'fn production() {', '    x();', '}', 'fn more() {', '    y();', '}'].join('\n');
  // The cfg+use lines strip; both production fns (6 code lines) stay. The old
  // scanner hunted to `production`'s `{` and stripped it → 3.
  assert.equal(prodLoc(src), 6);
});

test('REGRESSION: an unbalanced brace in a test string does not run the strip to EOF', () => {
  // `assert_eq!(s, "{")` - the unmatched `{` lives in a string; the real block
  // close still returns depth to 0. Production after the test block survives.
  const src = [
    'fn keep_me() {',
    '    real();',
    '}',
    '#[cfg(test)]',
    'mod tests {',
    '    fn t() {',
    '        assert_eq!(brace(), "{");',
    '    }',
    '}',
    'fn keep_me_too() {',
    '    also();',
    '}',
  ].join('\n');
  // keep_me (3) + keep_me_too (3) = 6
  assert.equal(prodLoc(src), 6);
});

test('strips #[cfg(all(test, …))] and #[cfg(any(test, …))] blocks', () => {
  const src = ['fn p() {}', '#[cfg(all(test, feature = "x"))]', 'mod a { fn t() {} }', '#[cfg(any(test, feature = "y"))]', 'mod b { fn u() {} }'].join('\n');
  assert.equal(prodLoc(src), 1); // only `fn p() {}`
});

test('does NOT strip #[cfg(not(test))] or #[cfg_attr(test, …)] (production)', () => {
  const src = ['#[cfg(not(test))]', 'fn prod_only() {', '    a();', '}', '#[cfg_attr(test, derive(Debug))]', 'struct S;'].join('\n');
  // all 6 lines are production (none stripped)
  assert.equal(prodLoc(src), 6);
});

test('multi-line raw string with braces does not corrupt the brace count', () => {
  const src = [
    'fn p() {',
    '    let sql = r#"',
    '        SELECT { not real braces }}}',
    '    "#;',
    '}',
    '#[cfg(test)]',
    'mod tests { fn t() {} }',
  ].join('\n');
  // production: fn p { ... } = the 5 lines of fn p (the raw-string lines are code lines inside it)
  assert.equal(prodLoc(src), 5);
});

test('char literal containing a brace is not counted as a real brace', () => {
  const masked = maskNonCode(["let open = '{';"]);
  assert.ok(!masked[0].includes('{'), "char-literal brace should be masked away");
});

test('multi-escape char literals (\\u{7b}, \\x7d) are masked, not leaked as braces', () => {
  const masked = maskNonCode(["let a = '\\u{7b}'; let b = '\\x7d';"]);
  assert.ok(!masked[0].includes('{') && !masked[0].includes('}'), 'escape braces must be masked');
});

test('REGRESSION: production code after a braceless cfg(test) on the SAME line survives', () => {
  // The braceless strip used to drop the whole physical line, taking the inline
  // production fn with it. rustfmt would split this, but the strip must be correct
  // in the dangerous (under-count) direction without relying on that.
  const src = ['let kept = mk();', '#[cfg(test)] use crate::y; fn prod() { go(); }'].join('\n');
  assert.equal(prodLoc(src), 2); // both lines count; the cfg+use part over-counts (safe)
});

test('unbalanced strip is reported, not silently dropped to EOF', () => {
  // A genuinely unclosed block (no real `}`): keep the lines + warn.
  const warnings = [];
  const masked = maskNonCode(['#[cfg(test)]', 'mod tests {', '    fn t() {', '        a();']);
  markCfgTestItems(masked, warnings);
  assert.equal(warnings.length, 1, 'expected one unbalanced-block warning');
});

// --- classify: the dead-band fixes -------------------------------------------

test('a small file may grow freely (no ratchet below warn)', () => {
  assert.equal(classify('f.rs', 210, 200).level, 'ok');
});

test('a file frozen at/above warn errors when it grows past baseline', () => {
  assert.equal(classify('f.rs', WARN_LOC + 100, WARN_LOC + 1).level, 'error');
});

test('REGRESSION: crossing the warn line via growth errors (was only a warn)', () => {
  // baseline 1400 (below warn), grows to 1600 (above warn) - previously slid by
  // with a non-fatal warning because the gate keyed on `base >= warn`.
  assert.equal(classify('f.rs', WARN_LOC + 100, WARN_LOC - 100).level, 'error');
});

test('REGRESSION: a NEW file born at/above warn errors (no new god-files)', () => {
  assert.equal(classify('new.rs', WARN_LOC + 400, undefined).level, 'error'); // ~1900, was a warn
});

test('a NEW file below warn is fine', () => {
  assert.equal(classify('new.rs', WARN_LOC - 1, undefined).level, 'ok');
});

test('a new/grown file over the hard cap reports the budget error', () => {
  const v = classify('big.rs', FAIL_LOC + 500, undefined);
  assert.equal(v.level, 'error');
  assert.match(v.message, new RegExp(`hard ${FAIL_LOC}`));
});

test('a grandfathered over-cap file only warns while it is not growing', () => {
  assert.equal(classify('project.rs', 7174, 7174).level, 'warn');
  assert.equal(classify('project.rs', 7000, 7174).level, 'warn'); // shrank - still just a warn
});

// --- discoverSrcRoots: workspace-aware scan roots -----------------------------

test('discoverSrcRoots tolerates a missing crates/ dir (pre-extraction layout)', () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'rust-file-size-roots-'));
  fs.mkdirSync(path.join(base, 'src'));
  assert.deepEqual(discoverSrcRoots(base), [path.join(base, 'src')]);
  fs.rmSync(base, { recursive: true, force: true });
});

test('discoverSrcRoots adds every crates/*/src after the root src', () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'rust-file-size-roots-'));
  fs.mkdirSync(path.join(base, 'src'));
  fs.mkdirSync(path.join(base, 'crates', 'zeta', 'src'), { recursive: true });
  fs.mkdirSync(path.join(base, 'crates', 'alpha', 'src'), { recursive: true });
  fs.mkdirSync(path.join(base, 'crates', 'no-src-yet'), { recursive: true }); // skipped: no src/
  assert.deepEqual(discoverSrcRoots(base), [
    path.join(base, 'src'),
    path.join(base, 'crates', 'alpha', 'src'),
    path.join(base, 'crates', 'zeta', 'src'),
  ]);
  fs.rmSync(base, { recursive: true, force: true });
});

// --- whole test files: measured, with their own thresholds (#576 E8-D1) ---------

test('TEST_FILE_RE: the names a whole-file test module may have', () => {
  for (const rel of ['a/tests.rs', 'a/foo_tests.rs', 'src/commands/command_tests.rs', 'a/anchor_fuzz.rs', 'a/test_helpers.rs']) {
    assert.ok(TEST_FILE_RE.test(rel), rel);
  }
  for (const rel of ['a/tests_util.rs', 'a/testsuite.rs', 'a/mod.rs', 'a/contests.rs']) assert.ok(!TEST_FILE_RE.test(rel), rel);
});

test('declaredUnderTestCfg: cfg(test) / all(test,..) / any(test,..) above `mod x;`, through stacked attributes', () => {
  assert.ok(declaredUnderTestCfg('#[cfg(test)]\nmod tests;', 'tests'));
  assert.ok(declaredUnderTestCfg('#[cfg(all(test, feature = "telemetry"))]\nmod cov_tests;', 'cov_tests'));
  assert.ok(declaredUnderTestCfg('#[cfg(any(test, feature = "test-support"))]\npub mod test_helpers;', 'test_helpers'));
  // A multi-line #[expect] whose reason string holds a `]` must not break the walk.
  const stacked = ['#[cfg(test)]', '#[expect(', '    clippy::unwrap_used,', '    reason = "panics are assertions [ok]"', ')]', 'pub(crate) mod tests;'].join('\n');
  assert.ok(declaredUnderTestCfg(stacked, 'tests'));
});

test('declaredUnderTestCfg: a plain, cfg(not(test)) or cfg_attr declaration is NOT a test module', () => {
  assert.ok(!declaredUnderTestCfg('mod parser_tests;', 'parser_tests'));
  assert.ok(!declaredUnderTestCfg('#[cfg(not(test))]\nmod parser_tests;', 'parser_tests'));
  assert.ok(!declaredUnderTestCfg('#[cfg_attr(test, derive(Debug))]\nmod parser_tests;', 'parser_tests'));
  assert.ok(!declaredUnderTestCfg('// #[cfg(test)]\nmod parser_tests;', 'parser_tests'), 'a commented-out cfg does not count');
  assert.ok(!declaredUnderTestCfg('#[cfg(test)]\nmod other;', 'parser_tests'));
});

test('isTestModule: a production file merely NAMED *_tests.rs is measured as production', () => {
  const files = {
    '/r/src/state/mod.rs': '#[cfg(test)]\nmod tests;\nmod sneaky_tests;\n',
    '/r/src/project.rs': '#[cfg(test)]\nmod tests;\n',
  };
  const read = (f) => {
    if (!(f in files)) throw new Error('ENOENT');
    return files[f];
  };
  const list = (d) => Object.keys(files).filter((f) => path.dirname(f) === d).map((f) => path.basename(f));
  assert.ok(isTestModule('/r/src/state/tests.rs', read, list), 'declared in the sibling mod.rs');
  assert.ok(isTestModule('/r/src/project/tests.rs', read, list), 'declared in the <dir>.rs next to the dir');
  assert.ok(!isTestModule('/r/src/state/sneaky_tests.rs', read, list), 'name matches, declaration is not test-gated');
  assert.ok(!isTestModule('/r/src/state/helpers.rs', read, list), 'name does not match');
});

test('testLoc counts every code line of a test file (nothing is stripped as cfg(test))', () => {
  const src = ['use super::*;', '', '// comment', '#[test]', 'fn t() {', '    let s = "{";', '}'].join('\n');
  assert.equal(testLoc(src), 5);
});

test('test-file thresholds: frozen at/above TEST_WARN, hard cap TEST_FAIL', () => {
  assert.equal(TEST_WARN_LOC, 3000);
  assert.equal(TEST_FAIL_LOC, 5000);
  // A frozen monolith may shrink but not grow by one line.
  assert.equal(classify('state/tests.rs', 21464, 21464, TEST_LIMITS).level, 'warn');
  assert.equal(classify('state/tests.rs', 21000, 21464, TEST_LIMITS).level, 'warn');
  assert.equal(classify('state/tests.rs', 21465, 21464, TEST_LIMITS).level, 'error');
  // A new test file may not be born at/above the test warn line, nor over the cap.
  assert.equal(classify('new_tests.rs', TEST_WARN_LOC, undefined, TEST_LIMITS).level, 'error');
  assert.match(classify('new_tests.rs', TEST_FAIL_LOC + 1, undefined, TEST_LIMITS).message, /hard 5000-LOC budget for test files/);
  // Below the test warn line a test file grows freely - well past the PRODUCTION cap.
  assert.equal(classify('small_tests.rs', 2900, 2500, TEST_LIMITS).level, 'ok');
  assert.equal(classify('small_tests.rs', 2900, 2500).level, 'error', 'the same numbers fail under production limits');
});

test('discoverExtraRoots finds build.rs, examples, tests, the OS helper and slice-rs when present', () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'rust-file-size-extra-'));
  try {
    for (const d of ['src-tauri/crates/telemetry', 'src-tauri/crates/models', 'src-tauri/examples', 'src-tauri/tests', 'e2e/agent/os/helper/src']) {
      fs.mkdirSync(path.join(base, d), { recursive: true });
    }
    fs.writeFileSync(path.join(base, 'src-tauri/build.rs'), 'fn main() {}');
    fs.writeFileSync(path.join(base, 'src-tauri/crates/telemetry/build.rs'), 'fn main() {}');
    assert.deepEqual(discoverExtraRoots(base), {
      files: ['src-tauri/build.rs', 'src-tauri/crates/telemetry/build.rs'],
      dirs: ['src-tauri/examples', 'e2e/agent/os/helper/src'],
      testDirs: ['src-tauri/tests'],
    });
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
  }
});

// --- --tighten: the refresh that never raises a row (#576 NF-W9E1-2) ----------

test('tightenRows lowers a row to its shrunk file and never raises one', () => {
  const recorded = { 'a.rs': 900, 'b.rs': 400, 'c.rs': 250 };
  const measured = { 'a.rs': 700, 'b.rs': 520, 'c.rs': 250 };
  // a shrank: lowered. b grew below warn: kept at 400 (--update would write
  // 520 - the loosening this mode exists to avoid). c unchanged.
  assert.deepEqual(tightenRows(recorded, measured), { 'a.rs': 700, 'b.rs': 400, 'c.rs': 250 });
});

test('tightenRows records an unrecorded file at its size and prunes a vanished one', () => {
  const recorded = { 'gone.rs': 300, 'kept.rs': 100 };
  const measured = { 'kept.rs': 100, 'new_tests.rs': 1234 };
  assert.deepEqual(tightenRows(recorded, measured), { 'kept.rs': 100, 'new_tests.rs': 1234 });
});

test('tightenRows output is sorted and every row is <= the recorded one', () => {
  const recorded = { 'z.rs': 10, 'm.rs': 5000, 'a.rs': 3 };
  const measured = { 'z.rs': 12, 'm.rs': 4999, 'a.rs': 1, 'b.rs': 7 };
  const rows = tightenRows(recorded, measured);
  assert.deepEqual(Object.keys(rows), ['a.rs', 'b.rs', 'm.rs', 'z.rs']);
  for (const [rel, loc] of Object.entries(rows)) {
    if (recorded[rel] !== undefined) assert.ok(loc <= recorded[rel], `${rel} raised to ${loc}`);
  }
});

test('unrecordedFiles names the measured files with no row, sorted', () => {
  assert.deepEqual(unrecordedFiles({ 'b.rs': 1, 'gone.rs': 2 }, { 'c.rs': 1, 'b.rs': 1, 'a.rs': 1 }), ['a.rs', 'c.rs']);
  assert.deepEqual(unrecordedFiles({ 'a.rs': 1 }, { 'a.rs': 9 }), []);
});

// [#576 RV10a-2] Recording a NEW file at/above its warn line would grandfather the
// file check mode refuses ("new file at/above warn", "exceeds the hard budget").
test('REGRESSION: tightenRows never records a NEW file at/above its warn line', () => {
  const measured = { 'big_new.rs': WARN_LOC, 'huge_new.rs': FAIL_LOC + 100, 'tiny_new.rs': 50, 'frozen.rs': 1650 };
  const recorded = { 'frozen.rs': 1600 };
  // frozen.rs has a row, so it keeps it (never raised); only tiny_new.rs is added.
  assert.deepEqual(tightenRows(recorded, measured), { 'frozen.rs': 1600, 'tiny_new.rs': 50 });
  const tests = { 'big_tests.rs': TEST_WARN_LOC, 'small_tests.rs': TEST_WARN_LOC - 1 };
  assert.deepEqual(tightenRows({}, tests, TEST_LIMITS), { 'small_tests.rs': TEST_WARN_LOC - 1 });
});

// A copy of the script in a throwaway tree resolves REPO_ROOT to that tree, so the
// CLI runs end to end against a fixture baseline.
function sizeGateFixture() {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'rust-size-tighten-'));
  fs.mkdirSync(path.join(base, 'scripts'));
  fs.mkdirSync(path.join(base, 'src-tauri', 'src'), { recursive: true });
  fs.copyFileSync(new URL('./check-rust-file-size.mjs', import.meta.url), path.join(base, 'scripts', 'check-rust-file-size.mjs'));
  const rs = (name, n) =>
    fs.writeFileSync(path.join(base, 'src-tauri', 'src', name), Array.from({ length: n }, (_, i) => `pub fn f${i}() {}\n`).join(''));
  rs('lib.rs', 10);
  rs('small.rs', 200);
  rs('big_new.rs', 1700);
  rs('huge_new.rs', 2100);
  rs('tiny_new.rs', 50);
  const baseline = path.join(base, 'src-tauri', 'file-size-baseline.json');
  fs.writeFileSync(
    baseline,
    JSON.stringify({
      warn_loc: WARN_LOC, fail_loc: FAIL_LOC, test_warn_loc: TEST_WARN_LOC, test_fail_loc: TEST_FAIL_LOC,
      files: { 'src-tauri/src/lib.rs': 10, 'src-tauri/src/small.rs': 300 },
      test_files: {},
    }, null, 2),
  );
  const run = (...args) =>
    spawnSync(process.execPath, [path.join(base, 'scripts', 'check-rust-file-size.mjs'), ...args], { encoding: 'utf8' });
  return { base, baseline, run };
}

test('REGRESSION: --tighten refuses a NEW file at/above warn with the split instruction and writes nothing', () => {
  const { base, baseline, run } = sizeGateFixture();
  try {
    const before = fs.readFileSync(baseline, 'utf8');
    const r = run('--tighten');
    assert.equal(r.status, 1, `--tighten must fail, got ${r.status}\n${r.stdout}${r.stderr}`);
    assert.match(r.stderr, /big_new\.rs: new production file at 1700 LOC \(>= warn 1500\).*Split it/);
    assert.match(r.stderr, /huge_new\.rs: 2100 LOC exceeds the hard 2000-LOC budget.*Split it/);
    assert.doesNotMatch(r.stderr, /tiny_new\.rs/);
    assert.equal(fs.readFileSync(baseline, 'utf8'), before, 'a refused --tighten must not write the baseline');
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
  }
});

test('REGRESSION: check mode tells only the files --tighten will record to run it', () => {
  const { base, run } = sizeGateFixture();
  try {
    const r = run();
    assert.equal(r.status, 1, 'the two new files at/above warn are check-mode errors');
    const note = r.stderr.split('\n').find((l) => l.includes('have no row')) ?? '';
    assert.match(note, /1 measured file\(s\) have no row \(src-tauri\/src\/tiny_new\.rs\); run --tighten/);
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
  }
});

// --- report ------------------------------------------------------------------

if (failures.length > 0) {
  console.error(`✗ check-rust-file-size.test: ${failures.length} failure(s):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ check-rust-file-size.test: ${passed} assertions passed.`);
