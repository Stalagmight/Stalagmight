// Fixture tests for the Playwright parallel-safety guard (scripts/check-e2e-parallel-safe.mjs).
//
// It was the one gate nothing pinned (#576 E7): a bare `process.exit(main())`
// with nothing exported, so a regression in its comment stripping could switch
// it off without any test noticing - and one did exist: a quoted `/*` opened a
// block comment that blanked the rest of the file. Every rule is pinned here
// with a planted pattern that MUST fire, next to the negatives that must not.
//
// Pure Node, no test framework (mirrors scripts/check-no-em-dash.test.mjs).
// Run: node scripts/check-e2e-parallel-safe.test.mjs  (also a CI smoke step).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { scan, stripComments, checkPremise, main, RULES, ALLOW } from './check-e2e-parallel-safe.mjs';

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

const SPEC = 'e2e-playwright/x.spec.ts';
// The rule fixtures below are snippets of a spec; each gets the guarded `test`
// import a real spec carries (#576 RV8b-E3-3), appended so line numbers hold.
const GUARD_IMPORT = "\nimport { test } from './fixtures/coverage';";
const scanSpec = (text, rel = SPEC) => scan([rel], () => text + GUARD_IMPORT);
const ids = (text, rel = SPEC) => scanSpec(text, rel).map((v) => v.ruleId);
/** A raw scan, for the guard's own cases. */
const rawIds = (text, rel = SPEC) => scan([rel], () => text).map((v) => v.ruleId);

// --- every rule fires on its pattern ---------------------------------------------

test('own-server and port-listen fire', () => {
  assert.deepEqual(ids('const s = http.createServer(handler);'), ['own-server']);
  assert.deepEqual(ids('server.listen(0);'), ['port-listen']);
});

test('disk-write fires on the original write calls', () => {
  for (const call of ['fs.writeFileSync(p, x)', 'await writeFile(p, x)', 'fs.appendFileSync(p, x)', 'fs.createWriteStream(p)', 'fs.copyFileSync(a, b)', 'fs.cpSync(a, b)', 'fs.renameSync(a, b)']) {
    assert.deepEqual(ids(call), ['disk-write'], call);
  }
});

test('disk-write now also fires on create / delete / link calls (#576 E7)', () => {
  for (const call of ['fs.mkdirSync(d)', 'await fsp.mkdir(d)', 'fs.unlinkSync(p)', 'await unlink(p)', 'fs.rmSync(d, { recursive: true })',
    'await fs.rm(d)', 'fs.rmdirSync(d)', 'fs.symlinkSync(a, b)', 'fs.truncateSync(p)', 'await fsp.rename(a, b)', 'await fs.cp(a, b)']) {
    assert.deepEqual(ids(call), ['disk-write'], call);
  }
});

test('disk-write fires on open-for-write, write, hard-link and chmod calls (#576 RV3-FE-12)', () => {
  for (const call of ["const fd = fs.openSync(p, 'w')", 'fs.writeSync(fd, buf)', 'fs.linkSync(a, b)', 'await fsp.link(a, b)',
    "await fs.promises.open(p, 'w')", 'fs.chmodSync(p, 0o644)', 'await fsp.chmod(p, 0o644)']) {
    assert.deepEqual(ids(call), ['disk-write'], call);
  }
});

test('the async open / link / chmod names need an fs receiver: window.open and a menu helper do not fire', () => {
  assert.deepEqual(ids("await page.evaluate(() => window.open('about:blank'));"), []);
  assert.deepEqual(ids('await menu.open();'), []);
  assert.deepEqual(ids("await link(page, 'Station A1');"), []);
});

// #576 RV4-FE-FEB-3: each of these wrote to disk without the gate seeing a call it knew.
const at = (text, rel = SPEC) => scanSpec(text, rel).map((v) => [v.ruleId, v.lineNo]);
const HIDDEN_WRITES = {
  "disk-write fires on bracket access: fs['writeFileSync'](p, x)": ["fs['writeFileSync'](p, x);", [['disk-write', 1]]],
  'disk-write fires on bracket access: fs["rmSync"](d)': ['fs["rmSync"](d, { recursive: true });', [['disk-write', 1]]],
  'disk-write fires on bracket access: a template-quoted fs[`mkdirSync`](d)': ['fs[`mkdirSync`](d);', [['disk-write', 1]]],
  "disk-write fires on bracket access to a short name on an fs receiver: fsp['open'](p, 'w')": ["await fsp['open'](p, 'w');", [['disk-write', 1]]],
  'disk-write fires on a destructured alias: const { writeFileSync: w } = fs': ['const { writeFileSync: w } = fs;\nw(p, x);', [['disk-write', 1]]],
  "disk-write fires on an import alias: import { writeFileSync as w } from 'node:fs'": ["import { writeFileSync as w } from 'node:fs';\nw(p, x);", [['disk-write', 1]]],
  'disk-write fires on a multi-line import, at the line of the name': ["import {\n  readFileSync,\n  writeFileSync as w,\n} from 'fs';\nw(p, x);", [['disk-write', 3]]],
  "disk-write fires on an imported async open: import { open } from 'fs/promises'": ["import { open } from 'fs/promises';\nawait open(p, 'w');", [['disk-write', 1]]],
  "disk-write fires on a required async open: const { open } = require('node:fs/promises')": ["const { open } = require('node:fs/promises');\nawait open(p, 'w');", [['disk-write', 1]]],
  'disk-write fires on a destructured async open: const { open } = fs.promises': ["const { open } = fs.promises;\nawait open(p, 'w');", [['disk-write', 1]]],
  'disk-write fires on the other short async names imported or destructured (link, chmod, rm, cp, rename, truncate)': [
    "import { link, chmod as ch } from 'node:fs/promises';\nconst { rm } = fsp;\nconst { cp, rename: mv } = fsPromises;\nconst { truncate } = await import('fs/promises');",
    [['disk-write', 1], ['disk-write', 2], ['disk-write', 3], ['disk-write', 4]],
  ],
  // #576 NF-W9FE-4: a nested destructure through `promises` hid the name.
  'disk-write fires on a nested destructure: const { promises: { open } } = fs': ["const { promises: { open } } = fs;\nawait open(p, 'w');", [['disk-write', 1]]],
  'disk-write fires on a nested destructure with an alias: const { promises: { writeFile: w } } = fs': ['const { promises: { writeFile: w } } = fs;\nawait w(p, x);', [['disk-write', 1]]],
  'disk-write fires on a multi-line nested destructure of require / import, at the line of each name': [
    "const {\n  readFileSync,\n  promises: {\n    rm,\n    rename: mv,\n  },\n} = require('node:fs');\nconst { promises: { link } } = await import('fs');\nconst { default: { promises: { chmod } } } = await import('node:fs');",
    [['disk-write', 4], ['disk-write', 5], ['disk-write', 8], ['disk-write', 9]],
  ],
};
for (const [name, [text, expected]] of Object.entries(HIDDEN_WRITES)) {
  test(name, () => {
    const got = at(text);
    assert.deepEqual(got, expected, `got ${JSON.stringify(got)}, want ${JSON.stringify(expected)}`);
  });
}

// #576 RV10b-G6: fs destructure evasions - a computed or quoted key, a typed
// pattern, and an fs module bound to a new name whose open / link / chmod is
// then called as a member.
const DESTRUCTURE_EVASIONS = {
  "disk-write fires on a computed key: const { ['open']: o } = fs.promises": ["const { ['open']: o } = fs.promises;\nawait o(p, 'w');", [['disk-write', 1]]],
  "disk-write fires on a quoted key: const { 'writeFile': w } = fsp": ["const { 'writeFile': w } = fsp;\nawait w(p, x);", [['disk-write', 1]]],
  'disk-write fires on a typed destructure: const { open }: typeof fs.promises = fs.promises': ['const { open }: typeof fs.promises = fs.promises;', [['disk-write', 1]]],
  "disk-write fires on an alias then a member call: const { promises: p } = fs; p.open(f, 'w')": [
    "const { promises: p } = fs;\nconst h = await p.open(f, 'w');\nawait p['chmod'](f, 0o600);",
    [['disk-write', 2], ['disk-write', 3]],
  ],
  'disk-write fires on fs bound to a new name, then a member open / link': [
    "import * as nodeFs from 'node:fs/promises';\nconst P = fs.promises;\nawait nodeFs.open(f, 'w');\nawait P.link(a, b);",
    [['disk-write', 3], ['disk-write', 4]],
  ],
  // #576 RV12-F4: a short name destructured from an alias of fs / fs.promises.
  "disk-write fires on a destructure from an fs alias: const p = fs.promises; const { open } = p": [
    "const p = fs.promises;\nconst { open } = p;\nawait open(f, 'w');",
    [['disk-write', 2]],
  ],
  'disk-write fires on a renamed destructure from an fs alias: const { chmod: c } = p': [
    "import * as nodeFs from 'node:fs/promises';\nconst { chmod: c, readFile } = nodeFs;\nawait c(f, 0o600);",
    [['disk-write', 2]],
  ],
  'disk-write fires on a destructure from an alias of an alias': [
    "const { promises: p } = fs;\nconst q = p;\nconst {\n  link,\n} = q;",
    [['disk-write', 4]],
  ],
};
for (const [name, [text, expected]] of Object.entries(DESTRUCTURE_EVASIONS)) {
  test(name, () => {
    const got = at(text);
    assert.deepEqual(got, expected, `got ${JSON.stringify(got)}, want ${JSON.stringify(expected)}`);
  });
}

test('an fs alias does NOT make a read or another object\'s open fire', () => {
  assert.deepEqual(ids("const { promises: p } = fs;\nawait p.readFile(f);\nawait p.stat(f);\nwindow.open(url);\nawait menu.open();"), []);
  assert.deepEqual(ids("const { ['readFile']: r } = fs.promises;\nconst { ['open']: o } = menu;"), []);
  // #576 RV12-F4: a read destructured from an alias, and a destructure from a member of
  // an alias that is not fs (`p.menu`), stay quiet.
  assert.deepEqual(ids('const p = fs.promises;\nconst { readFile, stat } = p;\nconst { open } = p.menu;\nconst { open: o } = pp;'), []);
});

test('a nested destructure does NOT fire on a read name, a non-fs source, or an object literal', () => {
  assert.deepEqual(ids('const { promises: { readFile, stat } } = fs;'), []);
  assert.deepEqual(ids('const { menu: { open } } = page;\nawait open();'), []);
  assert.deepEqual(ids("const opts = { promises: { open: true } };\nconst { a } = fs;"), []);
});

// #576 RV9b-7: the guarded import may name the module with its extension.
test('unguarded-test accepts the guarded import with a .js / .ts suffix', () => {
  for (const ext of ['.js', '.ts', '.mjs']) {
    assert.deepEqual(rawIds(`import { test, expect } from './fixtures/coverage${ext}';\ntest('a', async () => {});`), [], ext);
  }
  assert.deepEqual(rawIds("import { test } from './fixtures/coverage.json';"), ['unguarded-test'], 'not a module suffix');
});

test('the import / bracket checks do NOT fire on a read import, a namespace import, an object key, or a non-fs open', () => {
  assert.deepEqual(ids("import { readFileSync } from 'node:fs';"), []);
  assert.deepEqual(ids("import * as fs from 'fs';"), []);
  assert.deepEqual(ids('const handlers = {\n  open: async () => {},\n  rename: (x) => x,\n};'), []);
  assert.deepEqual(ids('const { open } = menu;\nawait open();'), []);
  assert.deepEqual(ids("import { open } from './helpers/menu';"), []);
  assert.deepEqual(ids("await menu['open']();\nitems['rename'](x);"), []);
  assert.deepEqual(ids("import { mkdirSync, writeFileSync } from 'fs';", 'e2e-playwright/fixtures/coverage.ts'), [], 'the ALLOW row still applies');
});

test('disk-write does NOT fire on a mock label, a UI helper named rename, or mkdtemp', () => {
  assert.deepEqual(ids("handlers['plugin:fs|writeFile'] = () => null;"), []);
  assert.deepEqual(ids("await rename(page, 'Section B');"), []);
  assert.deepEqual(ids("const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'x-'));"), []);
});

test('foreign-host-port fires on a literal or template port, not on :1420', () => {
  assert.deepEqual(ids("await page.goto('http://localhost:5173/');"), ['foreign-host-port']);
  assert.deepEqual(ids('await page.goto(`http://127.0.0.1:${port}/`);'), ['foreign-host-port']);
  assert.deepEqual(ids("await page.goto('http://localhost:1420/');"), []);
});

test('own-server fires on a WebSocket server too (#576 NF-FIX3B-1)', () => {
  assert.deepEqual(ids('const wss = new WebSocketServer({ port: 0 });'), ['own-server']);
  assert.deepEqual(ids('const wss = new WebSocket.Server({ noServer: true });'), ['own-server']);
  // A client socket to the shared server binds nothing.
  assert.deepEqual(ids("const ws = new WebSocket('ws://127.0.0.1:1420/');"), []);
});

test('foreign-host-port fires on 0.0.0.0 and on a concatenated port (#576 NF-FIX3B-1)', () => {
  assert.deepEqual(ids("await page.goto('http://0.0.0.0:5173/');"), ['foreign-host-port']);
  assert.deepEqual(ids('await page.goto(`http://0.0.0.0:${port}/`);'), ['foreign-host-port']);
  assert.deepEqual(ids("await page.goto('http://localhost:' + port);"), ['foreign-host-port']);
  assert.deepEqual(ids('await page.goto("http://127.0.0.1:" + server.port + "/");'), ['foreign-host-port']);
  assert.deepEqual(ids("await page.goto('http://localhost:' + 1420);"), []);
  assert.deepEqual(ids("await page.goto('http://0.0.0.0:1420/');"), []);
});

test('foreign-host-port fires on the IPv6 loopback and all-interfaces hosts (#576 NF-W4M-5)', () => {
  assert.deepEqual(ids("await page.goto('http://[::1]:5173/');"), ['foreign-host-port']);
  assert.deepEqual(ids('await page.goto(`http://[::1]:${port}/`);'), ['foreign-host-port']);
  assert.deepEqual(ids("await page.goto('http://[::1]:' + port);"), ['foreign-host-port']);
  assert.deepEqual(ids("const ws = new WebSocket('ws://[::]:5173/');"), ['foreign-host-port']);
  assert.deepEqual(ids("await page.goto('http://[::1]:1420/');"), []);
  assert.deepEqual(ids("await page.goto('http://[::]:1420/');"), []);
});

test('serial fires on both spellings', () => {
  assert.deepEqual(ids('test.describe.serial("x", () => {});'), ['serial']);
  assert.deepEqual(ids("test.describe.configure({ mode: 'serial' });"), ['serial']);
  assert.deepEqual(ids("test.describe.configure({ mode: 'parallel' });"), []);
});

// --- comment stripping -------------------------------------------------------------

test('REGRESSION: a quoted /* no longer blanks the rest of the file', () => {
  const text = ["const pattern = 'fixtures/*.dmp';", "fs.writeFileSync('/tmp/fixed-path.txt', 'x');", "test.describe.configure({ mode: 'serial' });"].join('\n');
  assert.deepEqual(ids(text), ['disk-write', 'serial']);
});

test('a // inside a string is not a comment (URL), a real trailing // is', () => {
  assert.deepEqual(ids("await page.goto('http://localhost:5173/'); // other"), ['foreign-host-port']);
  assert.deepEqual(ids("const ok = 1; // fs.writeFileSync('/tmp/x', 'y')"), []);
});

test('block comments are stripped across lines; a pattern in prose does not fire', () => {
  const text = ['/* never do this:', "  fs.writeFileSync('/tmp/x', 'y');", '  test.describe.serial() */', "fs.rmSync('/tmp/d');"].join('\n');
  const v = scanSpec(text);
  assert.deepEqual(v.map((x) => [x.ruleId, x.lineNo]), [['disk-write', 4]]);
});

test('REGRESSION (#576 RV3-FE-12): a /* inside a regex literal opens no block comment', () => {
  const text = ['const LEADING = /^\\/*/;', "fs.writeFileSync('/tmp/fixed-path.txt', 'x');", '/* end */'].join('\n');
  assert.deepEqual(scanSpec(text).map((x) => [x.ruleId, x.lineNo]), [['disk-write', 2]]);
  // Division and a regex after `=>` / `(` / `return` are told apart.
  const kept = stripComments(['const half = a / 2 / b; // note', 'xs.filter((s) => /\\/*x/.test(s));', 'return /"/.test(q); // q']);
  assert.deepEqual(kept, ['const half = a / 2 / b; ', 'xs.filter((s) => /\\/*x/.test(s));', 'return /"/.test(q); ']);
});

test('a multi-line template literal keeps a // inside it', () => {
  const lines = stripComments(['const html = `', '  <a href="http://localhost:5173/">', '`;']);
  assert.ok(lines[1].includes('localhost:5173'), lines[1]);
});

test('reports the original line as the snippet, and the 1-based line number', () => {
  const [v] = scanSpec("\n\n  fs.rmSync('/tmp/d'); // cleanup");
  assert.equal(v.lineNo, 3);
  assert.equal(v.snippet, "fs.rmSync('/tmp/d'); // cleanup");
});

// --- allowlist ---------------------------------------------------------------------

test('the coverage fixture is allowlisted for disk-write only', () => {
  assert.deepEqual(ALLOW.map((a) => [a.file, a.rule]), [['e2e-playwright/fixtures/coverage.ts', 'disk-write']]);
  assert.deepEqual(ids("fs.mkdirSync(coverageDir, { recursive: true });", 'e2e-playwright/fixtures/coverage.ts'), []);
  assert.deepEqual(ids("test.describe.serial('x', () => {});", 'e2e-playwright/fixtures/coverage.ts'), ['serial']);
  assert.ok(ALLOW.every((a) => a.reason && a.reason.length > 10));
});

// --- the fullyParallel premise -------------------------------------------------------

test('the premise holds only when the config sets fullyParallel: true in code', () => {
  assert.equal(checkPremise('export default defineConfig({ fullyParallel: true });'), null);
  assert.match(checkPremise('export default defineConfig({ fullyParallel: false });'), /fullyParallel: true/);
  assert.match(checkPremise('export default defineConfig({ /* fullyParallel: true */ });'), /fullyParallel: true/);
  assert.match(checkPremise(null), /not found/);
});

// --- main(): fail closed, on a real temp tree ---------------------------------------

test('main fails when e2e-playwright/ is missing, and on a symlinked spec', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pw-parallel-'));
  const quiet = () => {};
  try {
    assert.equal(main(root, quiet, quiet), 1, 'missing scan dir');
    fs.mkdirSync(path.join(root, 'e2e-playwright'));
    fs.writeFileSync(path.join(root, 'playwright.config.ts'), 'export default { fullyParallel: true };');
    fs.writeFileSync(path.join(root, 'e2e-playwright', 'a.spec.ts'), "import { test } from './fixtures/coverage';\ntest('a', () => {});");
    assert.equal(main(root, quiet, quiet), 0, 'clean tree');
    fs.writeFileSync(path.join(root, 'outside.ts'), "fs.writeFileSync('/tmp/x', 'y');");
    fs.symlinkSync('../outside.ts', path.join(root, 'e2e-playwright', 'b.spec.ts'));
    const lines = [];
    assert.equal(main(root, quiet, (m) => lines.push(m)), 1, 'symlinked spec');
    assert.ok(lines.some((l) => l.includes('[symlink] e2e-playwright/b.spec.ts')), lines.join('\n'));
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

// #576 RV8b-E3-3: the unmocked-IPC teardown guard is an auto fixture on the `test`
// fixtures/coverage.ts exports, so a spec that takes `test` from anywhere else runs
// with no guard and an unmocked command silently answers null.
const GUARDED = "import { test, expect } from './fixtures/coverage';\ntest('a', async () => {});";
test('unguarded-test fires on a spec that takes `test` from @playwright/test', () => {
  assert.deepEqual(rawIds("import { test, expect } from '@playwright/test';\ntest('a', async () => {});"), ['unguarded-test']);
  assert.deepEqual(rawIds("import { type Page, test } from '@playwright/test';\nimport { expect } from './fixtures/coverage';"), ['unguarded-test']);
  assert.deepEqual(rawIds(`import * as pw from '@playwright/test';\n${GUARDED}`), ['unguarded-test']);
  assert.deepEqual(rawIds(`import { test as base } from '@playwright/test';\n${GUARDED}`), ['unguarded-test']);
});

test('unguarded-test fires on a spec with no `test` from fixtures/coverage', () => {
  assert.deepEqual(rawIds("import { test, expect } from './helpers/app';\ntest('a', async () => {});"), ['unguarded-test']);
  assert.deepEqual(rawIds("import { expect } from './fixtures/coverage';\ntest('a', async () => {});"), ['unguarded-test']);
});

test('unguarded-test passes a guarded spec, a type-only import, a nested spec and a helper', () => {
  assert.deepEqual(rawIds(GUARDED), []);
  assert.deepEqual(rawIds(`import type { Locator } from '@playwright/test';\n${GUARDED}`), []);
  assert.deepEqual(rawIds("import {\n  test,\n  expect,\n} from '../fixtures/coverage';\ntest('a', async () => {});", 'e2e-playwright/sub/a.spec.ts'), []);
  assert.deepEqual(rawIds("import { test, expect, type Page } from '@playwright/test';", 'e2e-playwright/helpers/x.ts'), []);
});

test('the live tree passes', () => {
  assert.equal(main(undefined, () => {}, () => {}), 0);
});

test('every rule has an id, a regex and a reason', () => {
  for (const r of RULES) assert.ok(r.id && r.re instanceof RegExp && r.why, r.id);
});

// --- report --------------------------------------------------------------------------

if (failures.length > 0) {
  console.error(`✗ check-e2e-parallel-safe.test: ${failures.length} failure(s):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ check-e2e-parallel-safe.test: ${passed} checks passed.`);
