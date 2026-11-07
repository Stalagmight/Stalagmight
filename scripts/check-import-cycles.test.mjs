// Fixture tests for the value-import cycle gate (scripts/check-import-cycles.mjs).
//
// A gate that never fires proves nothing, so each edge kind is pinned with a
// planted two-module loop it MUST report, and each non-edge (type-only imports,
// inline `type` specifiers, a dynamic import) with a loop it must NOT. The last
// tests run the whole scan over a throwaway tree on disk, so the walk, the
// resolver and the exit code are covered too.
//
// Pure Node, no test framework (mirrors the sibling scripts/check-*.test.mjs).
// Run: node scripts/check-import-cycles.test.mjs  (pnpm lint:import-cycles:test; CI check job)

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { valueImports, resolveSpecifier, buildGraph, findCycles, exampleCycle, scan, main } from './check-import-cycles.mjs';

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

const cyclesOf = (texts) => findCycles(buildGraph(texts));
const pair = (aImportsB, bImportsA) => ({ 'src/a.ts': `${aImportsB}\nexport const a = 1;\n`, 'src/b.ts': `${bImportsA}\nexport const b = 1;\n` });

// --- must report ----------------------------------------------------------------

test('a two-module value cycle is reported', () => {
  assert.deepEqual(cyclesOf(pair("import { b } from './b';", "import { a } from './a';")), [['src/a.ts', 'src/b.ts']]);
});

test('every value edge kind closes a loop: default, namespace, side effect, re-export, export *', () => {
  for (const edge of [
    "import b from './b';",
    "import * as b from './b';",
    "import './b';",
    "export { b } from './b';",
    "export * from './b';",
    "import { type B, b } from './b';",
    "import b, { type B } from './b';",
  ]) {
    assert.equal(cyclesOf(pair(edge, "import { a } from './a';")).length, 1, edge);
  }
});

test('a longer loop is one component, with a concrete path back to its start', () => {
  const texts = {
    'src/store/x.ts': "import { y } from './y';\nexport const x = 1;",
    'src/store/y.ts': "import { z } from '../utils/z';\nexport const y = 1;",
    'src/utils/z.ts': "import { x } from '../store/x';\nexport const z = 1;",
    'src/leaf.ts': "import { x } from './store/x';",
  };
  const graph = buildGraph(texts);
  const [comp, ...rest] = findCycles(graph);
  assert.deepEqual(rest, []);
  assert.deepEqual(comp, ['src/store/x.ts', 'src/store/y.ts', 'src/utils/z.ts']);
  assert.deepEqual(
    exampleCycle(graph, comp).map((e) => `${e.from}:${e.line}->${e.to}`),
    ['src/store/x.ts:1->src/store/y.ts', 'src/store/y.ts:1->src/utils/z.ts', 'src/utils/z.ts:1->src/store/x.ts'],
  );
});

test('a module importing itself is reported', () => {
  assert.deepEqual(cyclesOf({ 'src/a.ts': "import { a as self } from './a';\nexport const a = 1;" }), [['src/a.ts']]);
});

test('.tsx files, index modules and explicit .ts extensions resolve', () => {
  const files = new Set(['src/a.tsx', 'src/dir/index.ts', 'src/c.ts']);
  assert.equal(resolveSpecifier('src/c.ts', './a', files), 'src/a.tsx');
  assert.equal(resolveSpecifier('src/c.ts', './dir', files), 'src/dir/index.ts');
  assert.equal(resolveSpecifier('src/dir/index.ts', '../c.ts', files), 'src/c.ts');
  assert.equal(resolveSpecifier('src/c.ts', 'react', files), null);
  assert.equal(resolveSpecifier('src/c.ts', './missing', files), null);
});

test('a multi-line import is one edge, at the line it starts on', () => {
  const imps = valueImports('src/a.ts', "// header\nimport {\n  b,\n  c,\n} from './b';\n");
  assert.deepEqual(imps, [{ spec: './b', line: 2 }]);
});

// --- must NOT report --------------------------------------------------------------

test('type-only imports and re-exports do not close a loop', () => {
  for (const edge of [
    "import type { B } from './b';",
    "import { type B } from './b';",
    "import { type B, type C } from './b';",
    "export type { B } from './b';",
    "export { type B } from './b';",
    "export type * from './b';",
  ]) {
    assert.deepEqual(cyclesOf(pair(edge, "import { a } from './a';")), [], edge);
  }
});

test('a dynamic import does not close a loop (it runs after both modules evaluated)', () => {
  assert.deepEqual(cyclesOf(pair("export const load = () => import('./b');", "import { a } from './a';")), []);
});

test('a one-way chain is clean', () => {
  assert.deepEqual(cyclesOf({ 'src/a.ts': "import { b } from './b';", 'src/b.ts': "import { c } from './c';", 'src/c.ts': 'export const c = 1;' }), []);
});

// --- the whole scan on disk ----------------------------------------------------------

function tree(files) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'import-cycles-'));
  for (const [rel, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    fs.writeFileSync(path.join(root, rel), text);
  }
  return root;
}
function quietly(fn) {
  const { log, error } = console;
  console.log = () => {};
  console.error = () => {};
  try {
    return fn();
  } finally {
    console.log = log;
    console.error = error;
  }
}

test('scan walks src/ (tests included, .d.ts and files outside src/ excluded) and main() fails on a loop', () => {
  const root = tree({
    'src/a.ts': "import { b } from './b';\nexport const a = 1;",
    'src/b.ts': "import { a } from './a';\nexport const b = 1;",
    'src/__tests__/a.test.ts': "import { a } from '../a';",
    'src/types.d.ts': "import { a } from './a';",
    'scripts/x.ts': "import { a } from '../src/a';",
  });
  try {
    const result = scan(root);
    assert.equal(result.files, 3);
    assert.deepEqual(result.cycles, [['src/a.ts', 'src/b.ts']]);
    assert.equal(quietly(() => main(root)), 1);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('main() passes a tree whose only loop is type-only', () => {
  const root = tree({
    'src/a.ts': "import type { B } from './b';\nexport const a: B | null = null;",
    'src/b.ts': "import { a } from './a';\nexport type B = typeof a;",
  });
  try {
    assert.equal(quietly(() => main(root)), 0);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

if (failures.length > 0) {
  console.error(`\n✗ check-import-cycles fixtures: ${failures.length} failed, ${passed} passed\n`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(`✓ check-import-cycles fixtures: ${passed} passed`);
