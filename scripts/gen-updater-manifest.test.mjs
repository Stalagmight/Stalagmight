// Fixture tests for the updater manifest generator
// (scripts/gen-updater-manifest.mjs).
//
// Every artifact and signature below is synthetic content written to a temp
// dir - no real fixture files, no real signing key.
//
// Pure Node, no test framework (mirrors scripts/check-updater-pubkey.test.mjs).
// Run: node scripts/gen-updater-manifest.test.mjs  (also a CI check step).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { buildManifest, parseArgs } from './gen-updater-manifest.mjs';

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

function makeDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'updater-manifest-'));
}

function write(dir, name, content = 'synthetic content') {
  fs.writeFileSync(path.join(dir, name), content);
}

// Shaped like a real minisign signature file (an "untrusted comment:" header
// line, then a base64 body line) - the structure gen-updater-manifest.mjs's
// readSig now validates, not just "non-empty".
function minisignSig(label) {
  return `untrusted comment: signature from minisign secret key\n${Buffer.from(`fake-${label}-signature-body`).toString('base64')}\ntrusted comment: fake trusted comment\n${Buffer.from(`fake-${label}-global-signature`).toString('base64')}\n`;
}

// Writes a full set of three-platform artifacts + sigs, with a name pattern
// that resembles the real bundler output (underscore-separated, per the
// `collect release artifacts` globs in release.yml).
function writeHappyPathArtifacts(dir, { linux = true, windows = true, darwin = true } = {}) {
  if (linux) {
    write(dir, 'Stalagmight_0.5.1_amd64.AppImage');
    write(dir, 'Stalagmight_0.5.1_amd64.AppImage.sig', minisignSig('linux'));
  }
  if (windows) {
    write(dir, 'Stalagmight_0.5.1_x64-setup.exe');
    write(dir, 'Stalagmight_0.5.1_x64-setup.exe.sig', minisignSig('windows'));
  }
  if (darwin) {
    write(dir, 'Stalagmight.app.tar.gz');
    write(dir, 'Stalagmight.app.tar.gz.sig', minisignSig('darwin'));
  }
}

const BASE = { version: '0.5.1', tag: 'v0.5.1', notes: 'Release notes.' };

test('happy path: all three platforms', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir);
  const manifest = buildManifest({ ...BASE, dir, includeDarwin: true });
  assert.equal(manifest.version, '0.5.1');
  assert.equal(manifest.notes, 'Release notes.');
  assert.ok(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}/.test(manifest.pub_date));
  assert.deepEqual(Object.keys(manifest.platforms), ['linux-x86_64', 'windows-x86_64', 'darwin-aarch64']);
  assert.equal(manifest.platforms['linux-x86_64'].signature, minisignSig('linux').trim());
  assert.equal(manifest.platforms['windows-x86_64'].signature, minisignSig('windows').trim());
  assert.equal(manifest.platforms['darwin-aarch64'].signature, minisignSig('darwin').trim());
  assert.equal(
    manifest.platforms['linux-x86_64'].url,
    'https://github.com/Stalagmight/Stalagmight/releases/download/v0.5.1/Stalagmight_0.5.1_amd64.AppImage',
  );
});

test('darwin excluded when --include-darwin false', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir);
  const manifest = buildManifest({ ...BASE, dir, includeDarwin: false });
  assert.deepEqual(Object.keys(manifest.platforms), ['linux-x86_64', 'windows-x86_64']);
});

test('darwin excluded by default is not requested even if present', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir, { darwin: true });
  const manifest = buildManifest({ ...BASE, dir, includeDarwin: false });
  assert.ok(!('darwin-aarch64' in manifest.platforms));
});

test('missing sig fails', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir);
  fs.rmSync(path.join(dir, 'Stalagmight_0.5.1_amd64.AppImage.sig'));
  assert.throws(() => buildManifest({ ...BASE, dir, includeDarwin: true }), /missing Linux update signature/);
});

test('empty sig fails', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir);
  write(dir, 'Stalagmight_0.5.1_amd64.AppImage.sig', '');
  assert.throws(() => buildManifest({ ...BASE, dir, includeDarwin: true }), /signature is empty/);
});

test('garbage-but-non-empty sig fails (not minisign-shaped)', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir);
  write(dir, 'Stalagmight_0.5.1_amd64.AppImage.sig', 'not a real signature, just some text\n');
  assert.throws(
    () => buildManifest({ ...BASE, dir, includeDarwin: true }),
    /does not look like a minisign signature/,
  );
});

test('a sig missing the base64 body line fails', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir);
  write(dir, 'Stalagmight_0.5.1_amd64.AppImage.sig', 'untrusted comment: signature from minisign secret key\n');
  assert.throws(
    () => buildManifest({ ...BASE, dir, includeDarwin: true }),
    /does not look like a minisign signature/,
  );
});

test('missing artifact fails, naming the platform', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir, { windows: false });
  assert.throws(() => buildManifest({ ...BASE, dir, includeDarwin: true }), /no Windows update artifact found/);
});

test('missing darwin artifact fails only when --include-darwin true', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir, { darwin: false });
  assert.throws(() => buildManifest({ ...BASE, dir, includeDarwin: true }), /no macOS update artifact found/);
  assert.doesNotThrow(() => buildManifest({ ...BASE, dir, includeDarwin: false }));
});

test('ambiguous match fails, naming the candidates', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir, { darwin: false });
  write(dir, 'Stalagmight_0.5.1_arm64.AppImage');
  write(dir, 'Stalagmight_0.5.1_arm64.AppImage.sig', minisignSig('linux-2'));
  let err;
  try {
    buildManifest({ ...BASE, dir, includeDarwin: false });
  } catch (e) {
    err = e;
  }
  assert.ok(err, 'expected buildManifest to throw');
  assert.match(err.message, /multiple Linux update artifacts matched/);
  assert.match(err.message, /Stalagmight_0\.5\.1_amd64\.AppImage/);
  assert.match(err.message, /Stalagmight_0\.5\.1_arm64\.AppImage/);
});

test('malformed --version fails', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir, { darwin: false });
  assert.throws(
    () => buildManifest({ ...BASE, version: 'not-a-version', dir, includeDarwin: false }),
    /not a valid version/,
  );
});

test('malformed --tag fails', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir, { darwin: false });
  assert.throws(
    () => buildManifest({ ...BASE, tag: 'not-a-tag', dir, includeDarwin: false }),
    /not a valid release tag/,
  );
});

test('a --tag missing the leading v fails', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir, { darwin: false });
  assert.throws(
    () => buildManifest({ ...BASE, tag: '0.5.1', dir, includeDarwin: false }),
    /not a valid release tag/,
  );
});

test('weekly version string passes through unchanged', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir, { darwin: false });
  const manifest = buildManifest({ ...BASE, version: '0.5.1-weekly.20260817', dir, includeDarwin: false });
  assert.equal(manifest.version, '0.5.1-weekly.20260817');
});

test('asset filenames are URL-encoded', () => {
  const dir = makeDir();
  write(dir, 'Stalagmight 0.5.1-setup.exe');
  write(dir, 'Stalagmight 0.5.1-setup.exe.sig', minisignSig('windows'));
  write(dir, 'Stalagmight_0.5.1_amd64.AppImage');
  write(dir, 'Stalagmight_0.5.1_amd64.AppImage.sig', minisignSig('linux'));
  const manifest = buildManifest({ ...BASE, dir, includeDarwin: false });
  assert.equal(
    manifest.platforms['windows-x86_64'].url,
    'https://github.com/Stalagmight/Stalagmight/releases/download/v0.5.1/Stalagmight%200.5.1-setup.exe',
  );
});

test('output key order is deterministic: version, notes, pub_date, platforms', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir);
  const manifest = buildManifest({ ...BASE, dir, includeDarwin: true });
  assert.deepEqual(Object.keys(manifest), ['version', 'notes', 'pub_date', 'platforms']);
});

test('missing notes falls back to an empty string, not a throw', () => {
  const dir = makeDir();
  writeHappyPathArtifacts(dir, { darwin: false });
  const { notes, ...rest } = BASE;
  const manifest = buildManifest({ ...rest, dir, includeDarwin: false });
  assert.equal(manifest.notes, '');
});

// --- parseArgs ---

test('parseArgs: happy path', () => {
  const opts = parseArgs([
    '--version', '0.5.1',
    '--tag', 'v0.5.1',
    '--dir', 'release-artifacts',
    '--notes', 'hello',
    '--out', 'latest-stable.json',
    '--include-darwin', 'true',
  ]);
  assert.deepEqual(opts, {
    version: '0.5.1',
    tag: 'v0.5.1',
    dir: 'release-artifacts',
    notes: 'hello',
    notesFile: undefined,
    out: 'latest-stable.json',
    includeDarwin: true,
  });
});

test('parseArgs: --include-darwin defaults to false when absent', () => {
  const opts = parseArgs(['--version', '0.5.1', '--tag', 'v0.5.1', '--dir', 'd', '--out', 'o.json']);
  assert.equal(opts.includeDarwin, false);
});

test('parseArgs: missing required flags are named together', () => {
  assert.throws(() => parseArgs(['--version', '0.5.1']), /--tag.*--dir.*--out|--tag[\s\S]*--dir[\s\S]*--out/);
});

test('parseArgs: --notes and --notes-file are mutually exclusive', () => {
  assert.throws(
    () =>
      parseArgs([
        '--version', '0.5.1',
        '--tag', 'v0.5.1',
        '--dir', 'd',
        '--out', 'o.json',
        '--notes', 'a',
        '--notes-file', 'b.md',
      ]),
    /mutually exclusive/,
  );
});

test('parseArgs: --include-darwin rejects anything but true/false', () => {
  assert.throws(
    () => parseArgs(['--version', '0.5.1', '--tag', 'v0.5.1', '--dir', 'd', '--out', 'o.json', '--include-darwin', 'yes']),
    /must be "true" or "false"/,
  );
});

if (failures.length > 0) {
  console.error(`✗ ${failures.length} failed, ${passed} passed`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(`✓ ${passed} updater-manifest generator tests passed`);
