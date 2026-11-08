// Fixture tests for the TS file-size ratchet (scripts/check-ts-file-size.mjs).
//
// Pins the prod-LOC strip heuristic against the inputs most likely to trip a
// naive line-comment/string scanner: a `//` sequence inside a string, a
// multi-line template literal with a nested `${ … }` expression (itself
// containing braces/strings/comments), a regex literal, and JSX text/comments.
// Also pins the same warn/hard-cap/grow classification contract as the Rust
// ratchet (scripts/check-rust-file-size.mjs), since the policy is identical.
//
// Pure Node, no test framework (mirrors check-rust-file-size.test.mjs's style).
// Run: node scripts/check-ts-file-size.test.mjs  (also a CI `check` step, pnpm
// lint:ts-file-size:test).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { prodLoc, classify, maskNonCode, WARN_LOC, FAIL_LOC, listScopedFiles, E2E_ROOTS, isGeneratedE2e } from './check-ts-file-size.mjs';
import { listSrcFiles, walkFiles, isTestPath, isGeneratedPath, symlinkProblem } from './lib/src-files.mjs';

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

// --- prodLoc: comment/string/template stripping ------------------------------

test('strips a trailing line comment, keeps the code on the same line', () => {
  const src = ['const a = 1; // trailing note', 'const b = 2;'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('a comment-only line does not count', () => {
  const src = ['function a() {', '  // just a note', '  return 1;', '}'].join('\n');
  assert.equal(prodLoc(src), 3); // fn a, return, }
});

test('a block comment spanning multiple lines does not count', () => {
  const src = ['const a = 1;', '/*', ' * multi-line doc comment', ' */', 'const b = 2;'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('REGRESSION: a `//` inside a string is NOT treated as a comment start', () => {
  // The naive scanner would see `//` in the URL and blank the rest of the line,
  // dropping the trailing `;` - a silent under-count.
  const src = ['const url = "http://example.com";', 'const kept = 1;'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('REGRESSION: a `/*`-shaped sequence inside a string does not open a fake block comment', () => {
  const src = ['const s = "/* not a comment */";', 'const kept = 1;'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('single and double quoted strings with escaped quotes are handled', () => {
  const src = ["const a = 'it\\'s fine';", 'const b = "she said \\"hi\\"";'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('a multi-line template literal body counts as production (tokei-style, not stripped)', () => {
  const src = ['const s = `', '  line one', '  line two', '`;'].join('\n');
  // all 4 physical lines carry non-whitespace masked content
  assert.equal(prodLoc(src), 4);
});

test('a template literal `${ … }` expression is scanned as real code, not string content', () => {
  const src = ['const s = `value: ${a + b}`;', 'const kept = 1;'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('REGRESSION: a comment inside a `${ … }` template expression still strips', () => {
  const src = ['const s = `${', '  // note', '  a', '}`;'].join('\n');
  // line 1: `const s = \`${` has code; line 2 comment-only strips; line 3 `a` counts; line 4 closer counts
  assert.equal(prodLoc(src), 3);
});

test('a nested object literal inside a template expression does not close the expression early', () => {
  const src = ['const s = `${JSON.stringify({ a: 1 })}`;', 'const kept = 1;'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('a JSX block comment `{/* … */}` blanks the comment but keeps surrounding braces as code', () => {
  const src = ['const el = <div>', '  {/* a note */}', '  <span>hi</span>', '</div>;'].join('\n');
  assert.equal(prodLoc(src), 4); // no line becomes fully blank (braces remain)
});

test('REGRESSION-KNOWN-LIMITATION: a literal `//` inside raw JSX text is treated as a line comment, undercounting the rest of the line', () => {
  // Documented limitation (see the KNOWN LIMITATION comment next to the `//`
  // handling in check-ts-file-size.mjs): the masker has no distinct JSX-text
  // context, so this line comment-strips from the `//` onward even though it
  // sits inside JSX text, not a real comment. Pinned as CURRENT behavior so a
  // future masker change surfaces the shift consciously rather than silently.
  const src = 'const el = <div>a // not b</div>;';
  assert.equal(prodLoc(src), 1);
});

test('a regex literal is recognized after an operator/paren, not treated as division', () => {
  const src = ['const re = /foo/g;', 'const kept = 1;'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('REGRESSION: division after an identifier is NOT mistaken for a regex start', () => {
  const src = ['const half = total / 2;', 'const kept = 1;'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('a char class inside a regex literal does not confuse the terminating slash', () => {
  const src = ['const re = /[a-z/]/g;', 'const kept = 1;'].join('\n');
  assert.equal(prodLoc(src), 2);
});

test('unterminated string at EOF is reported, not silently swallowed', () => {
  const warnings = [];
  maskNonCode(['const s = "unterminated'], warnings);
  assert.equal(warnings.length, 1, 'expected one unterminated-context warning');
});

// --- classify: same policy contract as the Rust ratchet -----------------------

test('a small file may grow freely (no ratchet below warn)', () => {
  assert.equal(classify('f.ts', 210, 200).level, 'ok');
});

test('a file frozen at/above warn errors when it grows past baseline', () => {
  assert.equal(classify('f.ts', WARN_LOC + 100, WARN_LOC + 1).level, 'error');
});

test('crossing the warn line via growth errors (not just a soft warn)', () => {
  assert.equal(classify('f.ts', WARN_LOC + 100, WARN_LOC - 100).level, 'error');
});

test('a NEW file born at/above warn errors (no new god-files)', () => {
  assert.equal(classify('new.tsx', WARN_LOC + 400, undefined).level, 'error');
});

test('a NEW file below warn is fine', () => {
  assert.equal(classify('new.tsx', WARN_LOC - 1, undefined).level, 'ok');
});

test('a new/grown file over the hard cap reports the budget error', () => {
  const v = classify('big.tsx', FAIL_LOC + 500, undefined);
  assert.equal(v.level, 'error');
  assert.match(v.message, new RegExp(`hard ${FAIL_LOC}`));
});

test('a grandfathered over-cap file only warns while it is not growing', () => {
  assert.equal(classify('DmpNewImportDialog.tsx', 3657, 3657).level, 'warn');
  assert.equal(classify('DmpNewImportDialog.tsx', 3600, 3657).level, 'warn'); // shrank - still just a warn
});

// --- scripts/lib/src-files.mjs: the one src/ lister every src gate uses (#576 E16)

test('isTestPath / isGeneratedPath: one predicate pair for every src gate', () => {
  for (const rel of ['src/a/__tests__/x.ts', 'src/a/x.test.tsx', 'src/a/x.spec.ts', 'src/test/setup.ts']) assert.ok(isTestPath(rel), rel);
  for (const rel of ['src/a/x.ts', 'src/testing/x.ts', 'src/a/contest.ts']) assert.ok(!isTestPath(rel), rel);
  assert.ok(isGeneratedPath('src/types/ipc/Shot.ts'));
  assert.ok(isGeneratedPath('src/types/ipc.generated.ts'));
  assert.ok(!isGeneratedPath('src/types/ipcHelpers.ts'));
});

test('listSrcFiles walks untracked files, prunes tests/generated/node_modules, returns symlinks unfollowed', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'src-files-'));
  try {
    const mk = (rel, text = 'x') => {
      fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
      fs.writeFileSync(path.join(root, rel), text);
    };
    mk('src/b.tsx');
    mk('src/a/c.ts');
    mk('src/a/readme.md');
    mk('src/__tests__/t.test.ts');
    mk('src/test/setup.ts');
    mk('src/types/ipc/Shot.ts');
    mk('src/node_modules/dep/index.ts');
    mk('elsewhere.ts');
    fs.symlinkSync('../elsewhere.ts', path.join(root, 'src', 'linked.ts'));
    fs.symlinkSync('missing.ts', path.join(root, 'src', 'dangling.ts'));
    const { files, symlinks } = listSrcFiles(root);
    assert.deepEqual(files, ['src/a/c.ts', 'src/b.tsx']);
    assert.deepEqual(symlinks, [
      { rel: 'src/dangling.ts', target: 'missing.ts' },
      { rel: 'src/linked.ts', target: '../elsewhere.ts' },
    ]);
    assert.match(symlinkProblem(symlinks[1]), /src\/linked\.ts is a symlink \(-> \.\.\/elsewhere\.ts\)/);
    assert.deepEqual(listSrcFiles(root, { exts: ['.tsx'] }).files, ['src/b.tsx']);
    assert.deepEqual(walkFiles(root, 'src', { exts: ['.md'] }).files, ['src/a/readme.md']);
    assert.throws(() => walkFiles(root, 'nope'), /ENOENT/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

// Every gate prints a symlink's target. One that is absolute or leaves the root
// names a path on the machine, not the checkout's text, so it prints resolved
// and folded (#576 W51, W58).
test('walkFiles gives a symlink target outside the root resolved and folded, one inside as written', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'src-files-out-'));
  const saved = { HOME: process.env.HOME, CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR };
  process.env.HOME = '/qzhome/qzuser';
  delete process.env.CARGO_TARGET_DIR;
  try {
    fs.mkdirSync(path.join(root, 'src', 'a'), { recursive: true });
    fs.writeFileSync(path.join(root, 'src', 'b.ts'), 'x');
    fs.symlinkSync('/qzhome/qzuser/CaveA/x.ts', path.join(root, 'src', 'abs.ts'));
    fs.symlinkSync('../../qzCaveA/x.ts', path.join(root, 'src', 'up.ts'));
    fs.symlinkSync('../b.ts', path.join(root, 'src', 'a', 'in.ts'));
    const { symlinks } = walkFiles(root, 'src', { exts: ['.ts'] });
    assert.deepEqual(symlinks, [
      { rel: 'src/a/in.ts', target: '../b.ts' },
      { rel: 'src/abs.ts', target: '~/CaveA/x.ts' },
      { rel: 'src/up.ts', target: '<tmp>/qzCaveA/x.ts' },
    ]);
    const printed = symlinks.map(symlinkProblem).join('\n');
    assert.doesNotMatch(printed, /qzhome|qzuser/);
    assert.ok(!printed.includes(os.tmpdir()), 'the temp dir prints as <tmp>');
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    fs.rmSync(root, { recursive: true, force: true });
  }
});

// --- scope: src/ plus the e2e trees (#576 E8-D2) ------------------------------

test('the ratchet measures e2e-playwright/ and e2e/ (specs included) but not generated replays', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ts-scope-'));
  try {
    const mk = (rel) => {
      fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
      fs.writeFileSync(path.join(root, rel), 'export {};\n');
    };
    for (const rel of ['src/a.ts', 'src/a.test.ts', 'e2e-playwright/x.spec.ts', 'e2e-playwright/mocks/tauri-ipc.ts',
      'e2e/agent/lib/y.ts', 'e2e/agent/lib/y.test.ts', 'e2e/agent/replays/gen-1.spec.ts', 'e2e/agent/node_modules/z.ts']) mk(rel);
    assert.deepEqual(listScopedFiles(root).files, [
      'src/a.ts', 'e2e-playwright/mocks/tauri-ipc.ts', 'e2e-playwright/x.spec.ts', 'e2e/agent/lib/y.test.ts', 'e2e/agent/lib/y.ts',
    ]);
    assert.deepEqual(E2E_ROOTS, ['e2e-playwright', 'e2e']);
    assert.ok(isGeneratedE2e('e2e/agent/replays/gen-1.spec.ts'));
    assert.ok(!isGeneratedE2e('e2e-playwright/replays.spec.ts'));
    // A missing e2e root is tolerated (partial checkout).
    fs.rmSync(path.join(root, 'e2e'), { recursive: true, force: true });
    assert.deepEqual(listScopedFiles(root).files, ['src/a.ts', 'e2e-playwright/mocks/tauri-ipc.ts', 'e2e-playwright/x.spec.ts']);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('the committed baseline freezes the IPC mock (the largest e2e file)', () => {
  const baseline = JSON.parse(fs.readFileSync(new URL('../ts-file-size-baseline.json', import.meta.url), 'utf8'));
  const mock = baseline.files['e2e-playwright/mocks/tauri-ipc.ts'];
  assert.ok(mock >= WARN_LOC, `tauri-ipc.ts baseline ${mock} should be frozen (>= ${WARN_LOC})`);
  assert.equal(classify('e2e-playwright/mocks/tauri-ipc.ts', mock + 1, mock).level, 'error', 'one more line must fail');
});

// --- report ------------------------------------------------------------------

if (failures.length > 0) {
  console.error(`✗ check-ts-file-size.test: ${failures.length} failure(s):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ check-ts-file-size.test: ${passed} assertions passed.`);
