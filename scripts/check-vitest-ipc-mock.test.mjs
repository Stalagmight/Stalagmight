// Fixture tests for the shared-IPC-mock gate (scripts/check-vitest-ipc-mock.mjs).
//
// Pins which vi.mock calls count as a factory mock of src/utils/ipc (every
// spelling of the specifier and of the factory), which do not (no factory,
// another module, prose), the allowlist's stale-row rule and main()'s tree
// walk. Pure Node, no test framework (mirrors scripts/check-e2e-no-sleep.test.mjs).
// Run: node scripts/check-vitest-ipc-mock.test.mjs  (also a CI check step).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { ALLOWLIST, findFactoryMocks, main, resolveSpecifier, scan } from './check-vitest-ipc-mock.mjs';

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

const DIALOG_TEST = 'src/components/Dialogs/__tests__/X.test.tsx';
const lines = (text, rel = DIALOG_TEST) => findFactoryMocks(text, rel).map((h) => h.lineNo);

test('the copied vi.hoisted + factory idiom fails, at the vi.mock line', () => {
  const text = [
    "import { vi } from 'vitest';",
    'const invokeIpcMock = vi.hoisted(() => vi.fn());',
    "vi.mock('../../../utils/ipc', () => ({",
    '  invokeIpc: invokeIpcMock,',
    '  getErrorMessage: (e: unknown, f?: string) => f ?? String(e),',
    '}));',
  ].join('\n');
  assert.deepEqual(lines(text), [3]);
});

test('every factory form counts: async, function, a vi.hoisted factory, an import() module', () => {
  assert.deepEqual(lines("vi.mock('../../../utils/ipc', async () => ({ invokeIpc: vi.fn() }));"), [1]);
  assert.deepEqual(lines("vi.mock('../../../utils/ipc', function () { return {}; });"), [1]);
  assert.deepEqual(lines("const { ipcFactory } = vi.hoisted(() => ({ ipcFactory: () => ({}) }));\nvi.mock('../../../utils/ipc', ipcFactory);"), [2]);
  assert.deepEqual(lines("vi.mock('../../../utils/ipc', () => import('../../../utils/__mocks__/ipc'));"), [1]);
  assert.deepEqual(lines("vi.doMock('../../../utils/ipc', () => ({ invokeIpc: vi.fn() }));"), [1]);
});

test('every spelling of the specifier counts: from utils/, with an extension, root-relative, typed import(), split lines', () => {
  assert.deepEqual(lines("vi.mock('../ipc', () => ({}));", 'src/utils/__tests__/x.test.ts'), [1]);
  assert.deepEqual(lines("vi.mock('./ipc', () => ({}));", 'src/utils/x.test.ts'), [1]);
  assert.deepEqual(lines('vi.mock("../../../utils/ipc.ts", () => ({}));'), [1]);
  assert.deepEqual(lines('vi.mock(`../../../utils/ipc`, () => ({}));'), [1]);
  assert.deepEqual(lines("vi.mock('/src/utils/ipc', () => ({}));"), [1]);
  assert.deepEqual(lines("vi.mock(import('../../../utils/ipc'), () => ({}));"), [1]);
  assert.deepEqual(lines("vi\n  .mock(\n    '../../../utils/ipc',\n    () => ({}),\n  );"), [1]);
  assert.deepEqual(lines("vi.mock('../../src/utils/ipc', () => ({}));", 'e2e/lib/x.test.ts'), [1]);
});

test('the shared mock (no factory) passes, with or without a trailing comma', () => {
  assert.deepEqual(lines("vi.mock('../../../utils/ipc');"), []);
  assert.deepEqual(lines("vi.mock('../../../utils/ipc',\n);"), []);
  assert.deepEqual(lines("vi.mock(import('../../../utils/ipc'));"), []);
});

test('a factory mock of any other module is out of scope', () => {
  assert.deepEqual(lines("vi.mock('../../../utils/ipcEvents', () => ({}));"), []);
  assert.deepEqual(lines("vi.mock('../../../utils/ipc/extra', () => ({}));"), []);
  assert.deepEqual(lines("vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));"), []);
  assert.deepEqual(lines("vi.mock('../ipc', () => ({}));", 'src/store/__tests__/x.test.ts'), [], 'src/store/ipc is not utils/ipc');
  assert.deepEqual(lines("vi.mock('ipc', () => ({}));"), [], 'a bare package name');
});

test('a factory mock in a comment or a string is not a call', () => {
  assert.deepEqual(lines("// vi.mock('../../../utils/ipc', () => ({ invokeIpc }))"), []);
  assert.deepEqual(lines("/*\n vi.mock('../../../utils/ipc', () => ({}));\n*/"), []);
  assert.deepEqual(lines("const doc = \"vi.mock('../../../utils/ipc', () => ({}))\";"), []);
});

test('two factory mocks in one file are two violations', () => {
  assert.deepEqual(lines("vi.mock('../../../utils/ipc', () => ({}));\nvi.doMock('../../../utils/ipc', () => ({}));"), [1, 2]);
});

test('resolveSpecifier folds extensions and /index, and ignores bare names', () => {
  assert.equal(resolveSpecifier('src/a/b/x.test.ts', '../../utils/ipc'), 'src/utils/ipc');
  assert.equal(resolveSpecifier('src/a/x.test.ts', '../utils/ipc.js'), 'src/utils/ipc');
  assert.equal(resolveSpecifier('src/a/x.test.ts', '../utils/ipc/index.ts'), 'src/utils/ipc');
  assert.equal(resolveSpecifier('src/a/x.test.ts', 'react-i18next'), null);
});

test('an allowlisted file passes; a row with no factory left (or no file) is stale and fails', () => {
  const files = { 'src/a.test.ts': "vi.mock('./utils/ipc', () => ({}));", 'src/b.test.ts': "vi.mock('./utils/ipc');" };
  const read = (rel) => files[rel];
  const ok = scan(['src/a.test.ts', 'src/b.test.ts'], read, { 'src/a.test.ts': 'reason' });
  assert.deepEqual(ok, { violations: [], stale: [], allowed: ['src/a.test.ts'] });
  const stale = scan(['src/a.test.ts', 'src/b.test.ts'], read, { 'src/a.test.ts': 'r', 'src/b.test.ts': 'r', 'src/gone.test.ts': 'r' });
  assert.deepEqual(stale.stale, ['src/b.test.ts', 'src/gone.test.ts']);
  const none = scan(['src/a.test.ts'], read, {});
  assert.deepEqual(none.violations, [{ rel: 'src/a.test.ts', lineNo: 1 }]);
});

test('every allowlist row carries a reason', () => {
  for (const [rel, reason] of Object.entries(ALLOWLIST)) assert.ok(reason.length >= 40, `${rel}: reason too short`);
});

test('main fails without src/, on a factory mock, on a stale row and on a symlink; passes a clean tree', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'vitest-ipc-mock-'));
  const quiet = () => {};
  const write = (rel, text) => {
    fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    fs.writeFileSync(path.join(root, rel), text);
  };
  try {
    // Repo-relative, never the absolute root: it names the checkout's location (#576 W51).
    const missing = [];
    assert.equal(main(root, quiet, (m) => missing.push(m)), 1, 'no src/');
    assert.deepEqual(missing, ['check-vitest-ipc-mock: src/ not found (run from the repo root).']);
    // A tree with only the shared idiom: the real ALLOWLIST rows are all stale here.
    write('src/utils/__tests__/a.test.ts', "vi.mock('../ipc');\n");
    for (const rel of Object.keys(ALLOWLIST)) write(rel, "vi.mock('/src/utils/ipc', () => ({}));\n");
    assert.equal(main(root, quiet, quiet), 0, 'clean tree with the allowlisted files');
    write('e2e/lib/b.test.ts', "vi.mock('../../src/utils/ipc', () => ({ invokeIpc: vi.fn() }));\n");
    assert.equal(main(root, quiet, quiet), 1, 'a factory mock outside src/');
    fs.rmSync(path.join(root, 'e2e'), { recursive: true });
    const first = Object.keys(ALLOWLIST)[0];
    write(first, "vi.mock('/src/utils/ipc');\n");
    assert.equal(main(root, quiet, quiet), 1, 'a stale allowlist row');
    write(first, "vi.mock('/src/utils/ipc', () => ({}));\n");
    assert.equal(main(root, quiet, quiet), 0);
    write('outside.ts', "vi.mock('./src/utils/ipc', () => ({}));\n");
    fs.symlinkSync('../../outside.ts', path.join(root, 'src', 'utils', 'linked.test.ts'));
    assert.equal(main(root, quiet, quiet), 1, 'a symlinked test file');
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

if (failures.length) {
  console.error(`✗ check-vitest-ipc-mock fixtures: ${failures.length} failed, ${passed} passed`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ check-vitest-ipc-mock fixtures: ${passed} passed`);
