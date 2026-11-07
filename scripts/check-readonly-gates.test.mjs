// Fixture tests for the canonical read-only gating tripwire (scripts/check-readonly-gates.mjs).
//
// Pure Node, no test framework (mirrors scripts/check-private-leaks.test.mjs).
// Run: node scripts/check-readonly-gates.test.mjs  (also a CI step, #275).

import assert from 'node:assert/strict';
import {
  countClosureAdjustedComparisons,
  stripLineComment,
  usesIsMapReadOnly,
  usesSpeleoReadOnly,
  CLOSURE_ADJUSTED_ALLOWLIST,
  DIRECT_READ_ONLY_ACCESS_DEFINITIONS,
  DIRECT_READ_ONLY_ACCESS_EXEMPT,
} from './check-readonly-gates.mjs';

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

// --- countClosureAdjustedComparisons --------------------------------------------

test('counts a raw equality comparison against closureAdjusted', () => {
  assert.equal(countClosureAdjustedComparisons("const x = geometryMode === 'closureAdjusted';"), 1);
  assert.equal(countClosureAdjustedComparisons('const x = geometryMode === "closureAdjusted";'), 1);
});

test('counts a raw inequality comparison against closureAdjusted', () => {
  assert.equal(countClosureAdjustedComparisons("const x = geometryMode !== 'closureAdjusted';"), 1);
});

test('counts a Yoda-form equality comparison (string on the left)', () => {
  assert.equal(countClosureAdjustedComparisons("if ('closureAdjusted' === geometryMode) a();"), 1);
});

test('counts a Yoda-form inequality comparison (string on the left)', () => {
  assert.equal(countClosureAdjustedComparisons("if ('closureAdjusted' !== geometryMode) a();"), 1);
});

test('counts multiple comparisons across lines', () => {
  const text = [
    "if (mode === 'closureAdjusted') a();",
    "if (mode === 'closureAdjusted') b();",
  ].join('\n');
  assert.equal(countClosureAdjustedComparisons(text), 2);
});

test('does not count a comparison inside a // line comment', () => {
  assert.equal(countClosureAdjustedComparisons("// geometryMode === 'closureAdjusted' is read-only"), 0);
});

test('does not count an assignment (not an equality check)', () => {
  assert.equal(countClosureAdjustedComparisons("geometryMode: 'closureAdjusted',"), 0);
});

test('does not count an unrelated equality comparison', () => {
  assert.equal(countClosureAdjustedComparisons("if (mode === 'measured') a();"), 0);
});

test('does not lose a real comparison that follows a // inside an earlier string literal (URL)', () => {
  // A naive `line.indexOf('//')` would truncate at the URL's "//" and silently
  // miss the real comparison later on the line - this is the exact bug fixed by
  // making stripLineComment string-literal-aware.
  const line = `const msg = "see http://example.com"; if (mode === 'closureAdjusted') a();`;
  assert.equal(countClosureAdjustedComparisons(line), 1);
});

// --- per-match counting and block comments (#576 E2-D2) --------------------------

test('REGRESSION: two comparisons on ONE line count as two (it counted lines)', () => {
  // Counting lines let a second raw gate ride along on an already-allowlisted
  // line: the exact-count allowlist could not see it.
  const line = "if (mode === 'closureAdjusted' || other !== 'closureAdjusted') a();";
  assert.equal(countClosureAdjustedComparisons(line), 2);
  assert.equal(countClosureAdjustedComparisons("const x = 'closureAdjusted' === m && m === 'closureAdjusted';"), 2);
});

test('REGRESSION: a comparison inside a /* block comment */ does not count', () => {
  assert.equal(countClosureAdjustedComparisons("/* geometryMode === 'closureAdjusted' is read-only */"), 0);
  const multi = ['/**', " * Was: if (geometryMode === 'closureAdjusted') return;", ' */', "if (m === 'closureAdjusted') a();"].join('\n');
  assert.equal(countClosureAdjustedComparisons(multi), 1);
});

test('a `case \'closureAdjusted\':` label counts as a comparison (switch bypass)', () => {
  const sw = ['switch (geometryMode) {', "  case 'closureAdjusted':", '    return;', '}'].join('\n');
  assert.equal(countClosureAdjustedComparisons(sw), 1);
  assert.equal(countClosureAdjustedComparisons("case 'measured':"), 0);
});

test('rule B ignores a mention inside a block comment, and still sees code after it', () => {
  assert.ok(!usesIsMapReadOnly('/* composed from isMapReadOnly plus the speleo lock */'));
  assert.ok(!usesSpeleoReadOnly(['/*', ' * see selectSpeleoReadOnly', ' */'].join('\n')));
  assert.ok(usesIsMapReadOnly('/* note */ useUIStore(isMapReadOnly);'));
});

test('a template-literal comparison counts like a quoted one, in every form (#576 RV3-FE-12)', () => {
  assert.equal(countClosureAdjustedComparisons('if (geometryMode === `closureAdjusted`) return;'), 1);
  assert.equal(countClosureAdjustedComparisons('if (`closureAdjusted` !== geometryMode) return;'), 1);
  assert.equal(countClosureAdjustedComparisons('switch (m) { case `closureAdjusted`: return; }'), 1);
  assert.equal(countClosureAdjustedComparisons("const a = m === `closureAdjusted` || m === 'closureAdjusted';"), 2);
  // Mismatched delimiters are not a comparison against the literal.
  assert.equal(countClosureAdjustedComparisons("const s = m === `closureAdjusted';"), 0);
});

test('a regex literal holding /* does not hide a later comparison (#576 RV3-FE-12)', () => {
  const text = ['export const LEADING_SLASHES = /^\\/*/;', "export const hidden = geometryMode === 'closureAdjusted';", '/* end */'].join('\n');
  assert.equal(countClosureAdjustedComparisons(text), 1);
});

test('a quoted /* does not hide a later real comparison', () => {
  const text = ["const glob = 'fixtures/*.dmp';", "if (m === 'closureAdjusted') a();"].join('\n');
  assert.equal(countClosureAdjustedComparisons(text), 1);
});

// --- stripLineComment -------------------------------------------------------------

test('stripLineComment leaves a URL inside a string literal intact', () => {
  const line = `const url = "http://example.com/closureAdjusted";`;
  assert.equal(stripLineComment(line), line);
});

test('stripLineComment strips a real trailing comment after a string containing a URL', () => {
  const line = `const url = "http://example.com"; // not a real comparison`;
  assert.equal(stripLineComment(line), `const url = "http://example.com"; `);
});

test('stripLineComment strips a plain trailing comment with no string literal', () => {
  assert.equal(stripLineComment("if (a) b(); // trailing note"), 'if (a) b(); ');
});

// --- usesIsMapReadOnly / usesSpeleoReadOnly -------------------------------------

test('detects isMapReadOnly as a bare identifier or a call', () => {
  assert.ok(usesIsMapReadOnly('useUIStore(isMapReadOnly)'));
  assert.ok(usesIsMapReadOnly('isMapReadOnly(useUIStore.getState())'));
  assert.ok(!usesIsMapReadOnly('const editingBlocked = useEditingBlocked();'));
});

test('a // comment that merely mentions the raw symbols does not trip rule B', () => {
  assert.ok(!usesIsMapReadOnly('// composed from isMapReadOnly plus the speleo lock'));
  assert.ok(!usesSpeleoReadOnly('// see selectSpeleoReadOnly in speleoStore'));
});

test('a real reference on a later line still trips rule B despite a comment-only mention', () => {
  const text = ['// isMapReadOnly is documented here', 'useUIStore(isMapReadOnly);'].join('\n');
  assert.ok(usesIsMapReadOnly(text));
});

test('detects selectSpeleoReadOnly as a bare identifier, a call, or an import', () => {
  assert.ok(usesSpeleoReadOnly('useSpeleoStore(selectSpeleoReadOnly)'));
  assert.ok(usesSpeleoReadOnly('selectSpeleoReadOnly(useSpeleoStore.getState())'));
  assert.ok(usesSpeleoReadOnly("import { selectSpeleoReadOnly } from '../store/speleoStore';"));
  assert.ok(!usesSpeleoReadOnly('const editingBlocked = useEditingBlocked();'));
});

// --- regression proof: this is the exact bug class #275 fixed ------------------

test('the pre-#275 DataTable.tsx pattern would have been flagged (regression proof, rule A)', () => {
  const buggy = "const geometryReadOnly = useUIStore((s) => s.geometryMode === 'closureAdjusted');";
  assert.equal(countClosureAdjustedComparisons(buggy), 1);
  assert.ok(!usesIsMapReadOnly(buggy)); // it didn't even reach for the canonical predicate
});

test('the pre-#275 short-circuiting composition would have been flagged (regression proof, rule B)', () => {
  // useUIStore(isMapReadOnly) || useSpeleoStore(selectSpeleoReadOnly) - the Rules-of-
  // Hooks-unsafe pattern the #275 blocker fixed by introducing useEditingBlocked().
  const buggy = 'const geometryReadOnly = useUIStore(isMapReadOnly) || useSpeleoStore(selectSpeleoReadOnly);';
  assert.ok(usesIsMapReadOnly(buggy));
  assert.ok(usesSpeleoReadOnly(buggy));
});

test('the fixed shared-hook call site passes both checks', () => {
  const fixed = 'const editingBlocked = useEditingBlocked();';
  assert.equal(countClosureAdjustedComparisons(fixed), 0);
  assert.ok(!usesIsMapReadOnly(fixed));
  assert.ok(!usesSpeleoReadOnly(fixed));
});

// --- allowlist hygiene -----------------------------------------------------------

test('the direct-access definitions set only names the two store modules and the helper', () => {
  assert.deepEqual(
    [...DIRECT_READ_ONLY_ACCESS_DEFINITIONS].sort(),
    ['src/hooks/useEditingBlocked.ts', 'src/store/speleoStore.ts', 'src/store/uiStore.ts'].sort(),
  );
});

test('every direct-access exemption carries a non-empty written reason', () => {
  for (const [file, reason] of Object.entries(DIRECT_READ_ONLY_ACCESS_EXEMPT)) {
    assert.ok(typeof reason === 'string' && reason.trim().length > 0, `${file}: exemption needs a reason`);
  }
});

test('every allowlisted count is a positive integer (no stray zero entries)', () => {
  for (const [file, count] of Object.entries(CLOSURE_ADJUSTED_ALLOWLIST)) {
    assert.ok(Number.isInteger(count) && count > 0, `${file}: allowlist count must be > 0`);
  }
});

// --- report -----------------------------------------------------------------------

if (failures.length > 0) {
  console.error(`✗ check-readonly-gates.test: ${failures.length} failure(s):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ check-readonly-gates.test: ${passed} checks passed.`);
