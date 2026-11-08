// Fixture tests for the updater pubkey/endpoint tripwire
// (scripts/check-updater-pubkey.mjs).
//
// tauri.conf.json commits the `plugins.updater` block (see
// docs/releasing.md); these fixtures pin what the gate refuses in it: a
// placeholder or malformed pubkey, an empty or non-https/non-GitHub
// endpoint. Also the block-absent pass, and the live config passing.
//
// Pure Node, no test framework (mirrors scripts/check-third-party-licenses.test.mjs).
// Run: node scripts/check-updater-pubkey.test.mjs  (also a CI check step).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { checkUpdaterConfig, REPO_ROOT } from './check-updater-pubkey.mjs';

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

function makeTree(conf) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'updater-pubkey-gate-'));
  fs.mkdirSync(path.join(root, 'src-tauri'));
  fs.writeFileSync(path.join(root, 'src-tauri', 'tauri.conf.json'), JSON.stringify(conf));
  return root;
}

// A real (throwaway, test-only) `tauri signer generate` output: base64 of an
// "untrusted comment:" line plus the 42-byte Ed25519 key line. The private
// half was discarded; this key signs nothing and ships nowhere.
const VALID_PUBKEY =
  'dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEQ2NjgyMzI1OTkzRjIyNkUKUldSdUlqK1pKU05vMXNrTkRjUW9nTFdFYXR2blRNTWI0SGFGUWNXdkNja29rYzRkWEVPaUs0czUK';
const VALID_ENDPOINT = 'https://github.com/Stalagmight/Stalagmight/releases/download/updater/latest-stable.json';

test('no plugins.updater block passes (pending state)', () => {
  const root = makeTree({});
  assert.deepEqual(checkUpdaterConfig(root), []);
});

test('valid pubkey and https GitHub endpoint pass', () => {
  const root = makeTree({ plugins: { updater: { pubkey: VALID_PUBKEY, endpoints: [VALID_ENDPOINT] } } });
  assert.deepEqual(checkUpdaterConfig(root), []);
});

test('placeholder pubkey fails', () => {
  const root = makeTree({
    plugins: { updater: { pubkey: 'UPDATER-PUBKEY-PENDING-SEE-ISSUE-579', endpoints: [VALID_ENDPOINT] } },
  });
  const problems = checkUpdaterConfig(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /pubkey is not a valid/);
});

test('empty pubkey fails', () => {
  const root = makeTree({ plugins: { updater: { pubkey: '', endpoints: [VALID_ENDPOINT] } } });
  assert.match(checkUpdaterConfig(root)[0], /pubkey is not a valid/);
});

test('base64-but-not-minisign pubkey fails (structure, not just encoding)', () => {
  const root = makeTree({ plugins: { updater: { pubkey: 'AAAAAAAA', endpoints: [VALID_ENDPOINT] } } });
  assert.match(checkUpdaterConfig(root)[0], /not a valid minisign public key/);
});

test('base64 of arbitrary text fails (no key line)', () => {
  const notAKey = Buffer.from('a valid minisign-style key body').toString('base64');
  const root = makeTree({ plugins: { updater: { pubkey: notAKey, endpoints: [VALID_ENDPOINT] } } });
  assert.match(checkUpdaterConfig(root)[0], /not a valid minisign public key/);
});

test('minisign structure with a truncated key line fails', () => {
  const truncated = Buffer.from(
    'untrusted comment: minisign public key: DEADBEEF\nRWRuIj+Z\n'
  ).toString('base64');
  const root = makeTree({ plugins: { updater: { pubkey: truncated, endpoints: [VALID_ENDPOINT] } } });
  assert.match(checkUpdaterConfig(root)[0], /not a valid minisign public key/);
});

test('non-https endpoint fails and names it', () => {
  const root = makeTree({
    plugins: { updater: { pubkey: VALID_PUBKEY, endpoints: ['http://github.com/x/y'] } },
  });
  const problems = checkUpdaterConfig(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /not https/);
});

test('non-github endpoint fails and names it', () => {
  const root = makeTree({
    plugins: { updater: { pubkey: VALID_PUBKEY, endpoints: ['https://evil.example.com/x'] } },
  });
  const problems = checkUpdaterConfig(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /not a github\.com URL/);
});

test('empty endpoints list fails', () => {
  const root = makeTree({ plugins: { updater: { pubkey: VALID_PUBKEY, endpoints: [] } } });
  assert.match(checkUpdaterConfig(root)[0], /endpoints is empty/);
});

test('github.com endpoint off the published manifest path fails', () => {
  const root = makeTree({
    plugins: {
      updater: {
        pubkey: VALID_PUBKEY,
        endpoints: ['https://github.com/Stalagmight/Stalagmight/releases/download/updater/latest.json'],
      },
    },
  });
  const problems = checkUpdaterConfig(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /does not point at the published manifest path/);
});

test('weekly manifest endpoint passes', () => {
  const root = makeTree({
    plugins: {
      updater: {
        pubkey: VALID_PUBKEY,
        endpoints: [
          'https://github.com/Stalagmight/Stalagmight/releases/download/updater/latest-weekly.json',
        ],
      },
    },
  });
  assert.deepEqual(checkUpdaterConfig(root), []);
});

test('missing/invalid tauri.conf.json fails loudly', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'updater-pubkey-gate-'));
  const problems = checkUpdaterConfig(root);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /missing or not valid JSON/);
});

// The gate must pass against the real repo: its committed block is sane.
test('live repo config passes', () => {
  assert.deepEqual(checkUpdaterConfig(REPO_ROOT), []);
});

if (failures.length > 0) {
  console.error(`✗ ${failures.length} failed, ${passed} passed`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(`✓ ${passed} updater-pubkey gate tests passed`);
