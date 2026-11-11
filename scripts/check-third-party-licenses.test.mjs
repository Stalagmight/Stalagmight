// Fixture tests for the third-party license bundle gate
// (scripts/check-third-party-licenses.mjs).
//
// The gate is a hash comparison, so the inputs it could get WRONG are pinned:
// a missing bundle, a bundle without the hashes comment, a stale hash for
// either lockfile, and the current pass state. Fixtures are built in a temp
// tree with hashes computed the same way the generator writes them, so the
// test does not depend on the repo's live lockfiles.
//
// Pure Node, no test framework (mirrors scripts/check-private-leaks.test.mjs).
// Run: node scripts/check-third-party-licenses.test.mjs  (also a CI check step).

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { checkBundle, REPO_ROOT } from './check-third-party-licenses.mjs';

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

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');

const GEN_V1 = 'generator-script-v1';
const ABOUT_V1 = 'about-toml-v1';

function makeTree({ cargo = 'cargo-lock-v1', pnpm = 'pnpm-lock-v1', gen = GEN_V1, about = ABOUT_V1, vendored = {}, bundle } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tpl-gate-'));
  fs.mkdirSync(path.join(root, 'src-tauri'));
  fs.mkdirSync(path.join(root, 'scripts'));
  fs.writeFileSync(path.join(root, 'src-tauri', 'Cargo.lock'), cargo);
  fs.writeFileSync(path.join(root, 'pnpm-lock.yaml'), pnpm);
  fs.writeFileSync(path.join(root, 'scripts', 'gen-third-party-licenses.mjs'), gen);
  fs.writeFileSync(path.join(root, 'src-tauri', 'about.toml'), about);
  for (const [rel, text] of Object.entries(vendored)) {
    fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    fs.writeFileSync(path.join(root, rel), text);
  }
  if (bundle !== undefined) fs.writeFileSync(path.join(root, 'THIRD-PARTY-LICENSES.md'), bundle);
  return root;
}

// The vendored-data hash the generator records: every third_party/ file as
// `relpath \0 content \0`, in sorted path order. An empty tree hashes nothing.
const vendoredHash = (files = {}) => {
  const h = createHash('sha256');
  for (const rel of Object.keys(files).sort()) h.update(rel + '\0' + files[rel] + '\0');
  return h.digest('hex');
};

// Mirrors the generator's layout: hashes comment (all four inputs, the
// vendored tree, and the body), then the body it hashes (every byte after
// the comment's closing `-->\n`).
const currentBundle = (cargo, pnpm, body = '\n## npm packages\n\nfixture body\n', vendored = {}) =>
  `# Third-party licenses\n\n<!-- lockfile-hashes\nCargo.lock=${sha256(cargo)}\npnpm-lock.yaml=${sha256(pnpm)}\ngen-third-party-licenses.mjs=${sha256(GEN_V1)}\nabout.toml=${sha256(ABOUT_V1)}\nvendored-data=${vendoredHash(vendored)}\nbundle-body=${sha256(body)}\n-->\n${body}`;

test('current bundle passes', () => {
  const root = makeTree({ bundle: currentBundle('cargo-lock-v1', 'pnpm-lock-v1') });
  assert.deepEqual(checkBundle(root), []);
});

test('a CRLF checkout of lockfiles and bundle still passes (autocrlf)', () => {
  const toCrlf = (s) => s.replace(/\n/g, '\r\n');
  const root = makeTree({
    cargo: toCrlf('cargo-lock-v1\n'),
    pnpm: toCrlf('pnpm-lock-v1\n'),
    bundle: toCrlf(currentBundle('cargo-lock-v1\n', 'pnpm-lock-v1\n')),
  });
  assert.deepEqual(checkBundle(root), []);
});

test('hand-edited body fails and says so', () => {
  const good = currentBundle('cargo-lock-v1', 'pnpm-lock-v1');
  const root = makeTree({ bundle: good.replace('fixture body', 'edited body') });
  const problems = checkBundle(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /edited by hand/);
});

test('bundle without a bundle-body hash fails', () => {
  const good = currentBundle('cargo-lock-v1', 'pnpm-lock-v1');
  const root = makeTree({ bundle: good.replace(/^bundle-body=[0-9a-f]{64}\n/m, '') });
  const problems = checkBundle(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /records no bundle-body hash/);
});

test('oversized bundle fails with the cap message, not "missing"', () => {
  const root = makeTree({ bundle: 'x'.repeat(33 * 1024 * 1024) });
  const problems = checkBundle(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /cap/);
});

test('missing bundle fails', () => {
  const root = makeTree();
  const problems = checkBundle(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /missing/);
});

test('bundle without a hashes comment fails', () => {
  const root = makeTree({ bundle: '# Third-party licenses\n\nhand-written\n' });
  assert.match(checkBundle(root)[0], /no lockfile-hashes comment/);
});

test('stale Cargo.lock hash fails and names the lockfile', () => {
  const root = makeTree({ bundle: currentBundle('OLD-cargo-lock', 'pnpm-lock-v1') });
  const problems = checkBundle(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /src-tauri\/Cargo\.lock changed/);
});

test('stale pnpm-lock hash fails and names the lockfile', () => {
  const root = makeTree({ bundle: currentBundle('cargo-lock-v1', 'OLD-pnpm-lock') });
  const problems = checkBundle(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /pnpm-lock\.yaml changed/);
});

test('both lockfiles stale reports both', () => {
  const root = makeTree({ bundle: currentBundle('OLD', 'ALSO-OLD') });
  assert.equal(checkBundle(root).length, 2);
});

test('a hash entry missing from the comment fails', () => {
  const good = currentBundle('cargo-lock-v1', 'pnpm-lock-v1');
  const root = makeTree({ bundle: good.replace(/^pnpm-lock\.yaml=[0-9a-f]{64}\n/m, '') });
  const problems = checkBundle(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /records no hash for pnpm-lock\.yaml/);
});

test('an edited generator script or about.toml fails', () => {
  const bundle = currentBundle('cargo-lock-v1', 'pnpm-lock-v1');
  const genRoot = makeTree({ bundle, gen: 'generator-script-v2' });
  assert.match(checkBundle(genRoot)[0], /gen-third-party-licenses\.mjs changed/);
  const aboutRoot = makeTree({ bundle, about: 'about-toml-v2' });
  assert.match(checkBundle(aboutRoot)[0], /about\.toml changed/);
});

test('a vendored notice edited without regenerating fails', () => {
  const vendored = { 'third_party/palettes/LICENSE-x': 'notice v1\n' };
  const bundle = currentBundle('cargo-lock-v1', 'pnpm-lock-v1', undefined, vendored);
  assert.deepEqual(checkBundle(makeTree({ bundle, vendored })), []);
  const edited = makeTree({ bundle, vendored: { 'third_party/palettes/LICENSE-x': 'notice v2\n' } });
  assert.match(checkBundle(edited)[0], /third_party\/ changed/);
  const added = makeTree({ bundle, vendored: { ...vendored, 'third_party/palettes/NOTICE-y': 'new\n' } });
  assert.match(checkBundle(added)[0], /third_party\/ changed/);
});

test('a dotfile under third_party/ does not change the vendored-data hash', () => {
  const vendored = { 'third_party/palettes/LICENSE-x': 'notice v1\n' };
  const bundle = currentBundle('cargo-lock-v1', 'pnpm-lock-v1', undefined, vendored);
  const withJunk = makeTree({ bundle, vendored: { ...vendored, 'third_party/palettes/.DS_Store': 'junk' } });
  assert.deepEqual(checkBundle(withJunk), []);
});

test('a bundle without a vendored-data hash fails', () => {
  const good = currentBundle('cargo-lock-v1', 'pnpm-lock-v1');
  const root = makeTree({ bundle: good.replace(/^vendored-data=[0-9a-f]{64}\n/m, '') });
  assert.match(checkBundle(root)[0], /no vendored-data hash/);
});

// The gate must pass against the real repo right now - a committed stale
// bundle should never survive to CI to begin with.
test('live repo bundle is current', () => {
  assert.deepEqual(checkBundle(REPO_ROOT), []);
});

if (failures.length > 0) {
  console.error(`✗ ${failures.length} failed, ${passed} passed`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(`✓ ${passed} third-party-license gate tests passed`);
