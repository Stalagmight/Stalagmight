// Fixture tests for the private-reference leak guard (scripts/check-private-leaks.mjs).
//
// A guard that never fires proves nothing - so each rule is pinned with a planted
// leak it MUST catch, and the load-bearing safety properties are pinned with the
// exact inputs that must NOT fire: the functional self-hosted-runner tokens kept
// in ci.yml (`i5-nuc-runner`, `NUC_RUNNER_READ_PAT`), the genericized placeholder
// paths, name lookalikes (Danielle/Nicobar/unicode), substring-boundary cases,
// and root-only public attribution.
//
// Pure Node, no test framework (mirrors scripts/check-rust-file-size.test.mjs).
// Run: node scripts/check-private-leaks.test.mjs  (also a CI leak-scan step).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import {
  findLeaks, findPathLeaks, isExcluded, isAttributionFile, RULES, gateFails, looksBinary, scan, report,
} from './check-private-leaks.mjs';
import { parseLsFilesStage, makeReader, hasBinaryExt, listTracked, MAX_SCAN_BYTES } from './lib/tracked-files.mjs';

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

// Helper: rule ids that fire on `text` for a given (non-attribution) file.
function firedIds(text, rel = 'src/example.ts') {
  return new Set(findLeaks(text, rel).map((v) => v.ruleId));
}

// --- maintainer-name: must fire -----------------------------------------------

test('maintainer-name fires on a name in a code comment', () => {
  assert.ok(firedIds('// ask Nico before changing this').has('maintainer-name'));
  assert.ok(firedIds('# Daniel owns the geometry layer').has('maintainer-name'));
});

test('maintainer-name fires on a name-bearing personal path (the #229 form)', () => {
  assert.ok(firedIds('const p = "/home/daniel/surveys/x.stalx";').has('maintainer-name'));
  assert.ok(firedIds('cp ~/build /Users/ndh/out').has('maintainer-name'));
  assert.ok(firedIds('C:\\Users\\ndh\\spring.stalx').has('maintainer-name'));
});

test('maintainer-name fires on the GitHub handle', () => {
  assert.ok(firedIds('see github.com/ndhailey/dev-tools').has('maintainer-name'));
});

test('maintainer-name fires on `_`- and digit-adjacent forms (the widened boundary)', () => {
  // `\b` missed these; the letter-lookaround catches them.
  assert.ok(firedIds('user daniel_saska pushed').has('maintainer-name'));
  assert.ok(firedIds('handle nico_dev').has('maintainer-name'));
  assert.ok(firedIds('account nico2 logged in').has('maintainer-name'));
});

// --- maintainer-name: must NOT fire (load-bearing negatives) -------------------

test('maintainer-name does NOT fire on letter-adjacent lookalikes', () => {
  // Danielle/Nicobar/unicode each contain a name substring but are letter-adjacent.
  assert.equal(firedIds('Danielle ran the unicode parser near Nicobar').size, 0);
});

test('maintainer-name `ndh` is path-anchored: /Users/ndh fires, a bare token does not', () => {
  assert.ok(firedIds('saved to /Users/ndh/out').has('maintainer-name'));
  // A standalone 3-char `ndh` token (column header, i18n key, hash) must NOT fire.
  assert.equal(firedIds('the ndh column maps to ndh_total').size, 0);
});

// --- private-cloud-infra: must fire / must not --------------------------------

test('private-cloud-infra fires on provider, env prefix (incl. digit-led), alias, host', () => {
  assert.ok(firedIds('# build runs on Scaleway').has('private-cloud-infra'));
  assert.ok(firedIds('export SCW_SECRET_KEY=abc').has('private-cloud-infra'));
  assert.ok(firedIds('SCW_2FA enabled').has('private-cloud-infra')); // digit-led, was missed by SCW_[A-Za-z]
  assert.ok(firedIds('ssh -i anzu remote').has('private-cloud-infra'));
  assert.ok(firedIds('rsync -avzP ./build cog:~/out').has('private-cloud-infra'));
  assert.ok(firedIds('scp file cog:/srv/x').has('private-cloud-infra'));
});

test('private-cloud-infra does NOT fire on boundary lookalikes / bare "cog"', () => {
  // `\b` boundary on scaleway, and `cog:` requires a path char - a bare cog is fine.
  assert.equal(firedIds('the descaleway helper turns a cog in the machine').size, 0);
});

// --- private-doc-or-tool: every alternative fires -----------------------------

test('private-doc-or-tool fires on each coordination doc / tool marker', () => {
  for (const marker of [
    'personaldocs', 'workingnotes', 'stalagmight-inflight', 'stalagmight-project-notes',
    'coord_board', 'ns-CLAUDE', 'stag-private', 'nico_sm_prioritized', 'nuc-i5',
  ]) {
    assert.ok(firedIds(`ref ~/x/${marker}/y`).has('private-doc-or-tool'), `expected ${marker} to fire`);
  }
});

// --- private-cave-path: must fire / must not ----------------------------------

test('private-cave-path fires on the maintainer cave-repo tree, in every path form', () => {
  assert.ok(firedIds('cp -r ~/src/cave/data/collab_test_data /tmp/x').has('private-cave-path'));
  assert.ok(firedIds('checked against /home/user/src/cave/SpeleoDB').has('private-cave-path'));
  assert.ok(firedIds('PathBuf::from(home).join("src/cave/data/qr")').has('private-cave-path'));
  assert.ok(firedIds('C:\\dev\\src\\cave\\data').has('private-cave-path'));
});

test('private-cave-path does NOT fire without the path segment boundary', () => {
  // `cave` must be a whole path segment: a longer segment or plain prose is fine.
  assert.equal(firedIds('the src/cavern.ts parser reads src/caves/ fixtures').size, 0);
  assert.equal(firedIds('this cave survey lives under src/parsers').size, 0);
});

// --- private-tracker ----------------------------------------------------------

test('private-tracker fires on the Plane references but not bare "plane"', () => {
  assert.ok(firedIds('tracked at app.plane.so/issue').has('private-tracker'));
  assert.ok(firedIds('| Plane ID | 1234 |').has('private-tracker'));
  assert.ok(!firedIds('project the point onto the survey plane').has('private-tracker'));
});

// --- the load-bearing SAFETY negatives ----------------------------------------

test('SAFETY: functional self-hosted-runner tokens kept in ci.yml do not fire', () => {
  // Intentionally retained in .github/workflows/ci.yml (the picker). `nuc-i5` is the
  // REVERSED tool-dir name, so neither token matches any rule. If a future rule edit
  // breaks this, the gate would red-CI on its own workflow file - this is the tripwire.
  assert.equal(findLeaks('name=="i5-nuc-runner" and status', 'ci.yml').length, 0);
  assert.equal(findLeaks('GH_TOKEN: ${{ secrets.NUC_RUNNER_READ_PAT }}', 'ci.yml').length, 0);
});

test('SAFETY: genericized placeholder paths do not fire', () => {
  assert.equal(firedIds("ensureExtension('/home/v1.0/myfile', 'stalx')").size, 0);
  assert.equal(firedIds('save to /home/user/out and /Users/x/in').size, 0);
});

// --- attribution exemption is ROOT-ONLY ---------------------------------------

test('maintainer-name is suppressed only in ROOT attribution files', () => {
  // Root README/AUTHORS may credit a maintainer by name...
  assert.equal(findLeaks('Built by Nico and Daniel.', 'README.md').length, 0);
  assert.equal(findLeaks('Maintainers: Nico, Daniel.', 'AUTHORS.md').length, 0);
  // ...but a NESTED code-adjacent README is NOT an attribution file - name fires.
  assert.ok(
    findLeaks('// TODO(daniel): fix this', 'src/components/SpreadsheetGrid/README.md')
      .some((v) => v.ruleId === 'maintainer-name'),
  );
  // ...and a docs/ AUTHORS is nested, so root-only means it is NOT exempt either.
  assert.ok(findLeaks('Thanks Nico!', 'docs/AUTHORS.md').some((v) => v.ruleId === 'maintainer-name'));
  // Non-name rules still fire even in a root attribution file.
  assert.ok(
    findLeaks('credits - built on Scaleway', 'README.md').some((v) => v.ruleId === 'private-cloud-infra'),
  );
});

test('isAttributionFile matches ROOT basenames only, case-insensitively', () => {
  assert.ok(isAttributionFile('README.md'));
  assert.ok(isAttributionFile('AUTHORS.md'));
  assert.ok(isAttributionFile('CHANGELOG.md'));
  assert.ok(!isAttributionFile('docs/AUTHORS.md')); // nested - not exempt
  assert.ok(!isAttributionFile('src/components/SpreadsheetGrid/README.md'));
  assert.ok(!isAttributionFile('src/store/uiStore.ts'));
});

// --- exclusions ---------------------------------------------------------------

test('isExcluded covers vendor, lockfiles, generated, binary, and self', () => {
  assert.ok(isExcluded('src-tauri/vendor/compass_data/MODIFICATIONS.md'));
  assert.ok(isExcluded('src-tauri/Cargo.lock'));
  assert.ok(isExcluded('pnpm-lock.yaml'));
  assert.ok(isExcluded('src/types/ipc.generated.ts'));
  assert.ok(isExcluded('src/types/ipc/Project.ts'));
  assert.ok(isExcluded('docs/screenshot.png'));
  assert.ok(isExcluded('docs/SCREENSHOT.PNG')); // extension lowercased
  assert.ok(isExcluded('scripts/check-private-leaks.mjs'));
  assert.ok(isExcluded('scripts/check-private-leaks.test.mjs'));
  assert.ok(isExcluded('e2e/fixtures/test_simple.tml')); // Ariane .tml is a binary zip container (#241)
});

test('isExcluded does NOT skip text formats that can carry a leak', () => {
  assert.ok(!isExcluded('src/assets/icon.svg')); // text - now scanned
  assert.ok(!isExcluded('e2e-playwright/__snapshots__/x.snap')); // text - now scanned
  assert.ok(!isExcluded('Makefile')); // extensionless - scanned
  assert.ok(!isExcluded('src/store/uiStore.ts'));
  assert.ok(!isExcluded('docs/roadmap.md'));
});

// --- fail closed on an unscannable file (Copilot review, #232) ----------------

test('gateFails: clean only with zero violations AND zero skips', () => {
  assert.equal(gateFails([], []), false);
  // An unscanned (oversized/unreadable) file fails the gate even with no markers -
  // else a marker could be smuggled inside a file the scanner declines to read.
  assert.equal(gateFails([], [{ rel: 'big.md', reason: '9.0 MB > 5 MB cap' }]), true);
  assert.equal(gateFails([{ ruleId: 'maintainer-name' }], []), true);
});

test('looksBinary flags a NUL byte so an undecidable file fails closed (#241)', () => {
  // A file whose extension is NOT in BINARY_EXT (so it reaches this check) but that
  // contains a NUL is undecidable: the scan must treat it as skipped -> gate failure,
  // not silently pass it. `looksBinary` is what routes it there.
  assert.equal(looksBinary(Buffer.from('hello\x00world', 'utf8')), true);
  assert.equal(looksBinary(Buffer.from('plain utf8 text, no nul', 'utf8')), false);
  // A NUL past the 8000-byte sniff window is not detected (bounded scan) -- documents
  // the known limit rather than asserting perfection.
  const late = Buffer.concat([Buffer.alloc(8000, 0x61), Buffer.from([0x00])]);
  assert.equal(looksBinary(late), false);
});

// --- rule hygiene -------------------------------------------------------------

test('every rule regex carries the global flag (required by the exec loop)', () => {
  for (const r of RULES) assert.ok(r.re.global, `${r.id} regex must be /g`);
});

// --- scan(): wiring through an injected reader (#576 E16 / NF-LINK) -------------

const buf = (s) => Buffer.from(s, 'utf8');

test('scan reports a planted marker and passes a clean file', () => {
  const r = scan(['a.ts', 'b.ts'], (rel) => buf(rel === 'a.ts' ? 'ask Daniel' : 'clean'));
  assert.equal(r.scanned, 2);
  assert.deepEqual(r.violations.map((v) => [v.rel, v.ruleId]), [['a.ts', 'maintainer-name']]);
  assert.equal(report(r, () => {}, () => {}), 1);
  assert.equal(report(scan(['b.ts'], () => buf('clean')), () => {}, () => {}), 0);
});

test('a symlink is scanned as its link text and NEVER followed', () => {
  // The old gate read the link's target file with readFileSync: a link out of
  // the repo had foreign content scanned, and the link text was never checked.
  const r = scan([{ rel: 'docs/notes.md', symlink: true }], () => ({ symlink: '../personaldocs/notes.md' }));
  assert.deepEqual(r.violations.map((v) => [v.ruleId, v.link]), [['private-doc-or-tool', true]]);
  const lines = [];
  report(r, () => {}, (m) => lines.push(m));
  assert.ok(lines.some((l) => l.includes('docs/notes.md (symlink target):1')), lines.join('\n'));
});

test('a dangling symlink no longer fails as "unreadable": its clean text passes', () => {
  const r = scan([{ rel: 'x.md', symlink: true }], () => ({ symlink: 'nowhere.md' }));
  assert.equal(r.skipped.length, 0);
  assert.deepEqual(r.symlinks, ['x.md']);
  assert.equal(report(r, () => {}, () => {}), 0);
});

test('a symlink with a binary extension is still scanned (its content is text)', () => {
  const r = scan([{ rel: 'docs/logo.png', symlink: true }], () => ({ symlink: '/home/daniel/logo.png' }));
  assert.equal(r.violations.length, 1);
  assert.ok(isExcluded('docs/logo.png'), 'a regular .png is still excluded');
});

test('oversized, unreadable, NUL-bearing and unrecognised reads each fail closed', () => {
  const big = scan(['big.md'], () => Buffer.alloc(6 * 1024 * 1024, 0x61));
  assert.match(big.skipped[0].reason, /cap/);
  const gone = scan(['gone.md'], () => { throw Object.assign(new Error('x'), { code: 'ENOENT' }); });
  assert.match(gone.skipped[0].reason, /ENOENT/);
  const nul = scan(['weird.ts'], () => Buffer.from([0x61, 0x00]));
  assert.match(nul.skipped[0].reason, /NUL/);
  const odd = scan(['odd.ts'], () => null);
  assert.equal(odd.skipped.length, 1);
  for (const r of [big, gone, nul, odd]) assert.equal(report(r, () => {}, () => {}), 1);
});

test('scan never reads an excluded path (exclusion precedes the read)', () => {
  const read = [];
  scan(['src-tauri/vendor/x.rs', 'docs/a.png', 'ok.ts'], (rel) => {
    read.push(rel);
    return buf('clean');
  });
  assert.deepEqual(read, ['ok.ts']);
});

// --- paths are scanned too (#576 E18(e)) -------------------------------------------

test('a marker only in the PATH fails, though the content is clean', () => {
  const r = scan(['docs/personaldocs-plan.md'], () => buf('clean ascii text'));
  assert.deepEqual(r.violations.map((v) => [v.ruleId, v.path]), [['private-doc-or-tool', true]]);
  const lines = [];
  assert.equal(report(r, () => {}, (m) => lines.push(m)), 1);
  assert.ok(lines.some((l) => l.includes('docs/personaldocs-plan.md (path)')), lines.join('\n'));
});

test('a marker in the name of an EXCLUDED file (binary, vendored) still fails', () => {
  const read = [];
  const r = scan(['docs/daniel-headshot.png', 'src-tauri/vendor/x/anzu.rs'], (rel) => { read.push(rel); return buf(''); });
  assert.deepEqual(read, [], 'excluded content is still never read');
  assert.deepEqual(r.violations.map((v) => v.ruleId).sort(), ['maintainer-name', 'private-cloud-infra']);
});

test('the root-attribution exemption covers content, never the path', () => {
  assert.equal(findPathLeaks('README.md').length, 0);
  // A root attribution file may credit a name in its CONTENT...
  assert.equal(scan(['CHANGELOG.md'], () => buf('thanks Daniel')).violations.length, 0);
  // ...but a path is checked without that exemption.
  assert.equal(findPathLeaks('docs/daniel/README.md').length, 1);
});

// --- scripts/lib/tracked-files.mjs: the shared reader both guards use -------------

test('parseLsFilesStage reads the symlink mode and de-duplicates conflict stages', () => {
  const out = '100644 aaa 0\tREADME.md\0' + '120000 bbb 0\tAGENTS.md\0'
    + '100644 ccc 1\tsrc/x.ts\0' + '100644 ddd 2\tsrc/x.ts\0';
  assert.deepEqual(parseLsFilesStage(out), [
    { rel: 'README.md', symlink: false },
    { rel: 'AGENTS.md', symlink: true },
    { rel: 'src/x.ts', symlink: false },
  ]);
});

test('makeReader returns a link\'s target string (live or dangling) and a file\'s bytes', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tracked-files-'));
  try {
    fs.writeFileSync(path.join(root, 'real.md'), 'ask Daniel');
    fs.symlinkSync('real.md', path.join(root, 'live.md'));
    fs.symlinkSync('missing.md', path.join(root, 'dangling.md'));
    const read = makeReader(root);
    assert.deepEqual(read('live.md'), { symlink: 'real.md' }, 'a live link must not be followed');
    assert.deepEqual(read('dangling.md'), { symlink: 'missing.md' });
    assert.equal(read('real.md').toString('utf8'), 'ask Daniel');
    // End to end: the live link's text is clean even though its target is not.
    const r = scan([{ rel: 'live.md', symlink: true }], read);
    assert.equal(r.violations.length, 0);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

// git's stderr reaches ours folded, and a failing git still throws what
// execFileSync throws (#576 W58): its dubious-ownership text names the checkout.
// A fake git first on PATH stands in for the real one.
test('listTracked prints git\'s stderr folded and still throws its exit status', () => {
  if (process.platform === 'win32') return; // the fake git is a shell script
  const repo = path.resolve(import.meta.dirname, '..');
  const bin = fs.mkdtempSync(path.join(os.tmpdir(), 'fake-git-'));
  fs.writeFileSync(
    path.join(bin, 'git'),
    `#!/bin/sh\necho "fatal: detected dubious ownership in repository at '${repo}'" >&2\nexit 128\n`,
    { mode: 0o755 },
  );
  const savedPath = process.env.PATH;
  const write = process.stderr.write;
  const written = [];
  let err;
  process.env.PATH = `${bin}${path.delimiter}${savedPath}`;
  process.stderr.write = (chunk) => (written.push(String(chunk)), true);
  try {
    listTracked(repo);
  } catch (e) {
    err = e;
  } finally {
    process.stderr.write = write;
    process.env.PATH = savedPath;
    fs.rmSync(bin, { recursive: true, force: true });
  }
  assert.equal(written.join(''), "fatal: detected dubious ownership in repository at '<repo>'\n");
  assert.ok(err instanceof Error, 'a failing git must still throw');
  assert.equal(err.status, 128);
  assert.equal(err.message.split('\n')[0], 'Command failed: git ls-files -s -z');
});

test('both whole-tree guards share one binary-extension set and one size cap', () => {
  assert.ok(hasBinaryExt('a/b.PNG'));
  assert.ok(!hasBinaryExt('a/b.svg'));
  assert.ok(!hasBinaryExt('a.d/Makefile'), 'a dot in a DIRECTORY name is not an extension');
  assert.equal(MAX_SCAN_BYTES, 5 * 1024 * 1024);
});

// --- report -------------------------------------------------------------------

if (failures.length > 0) {
  console.error(`✗ check-private-leaks.test: ${failures.length} failure(s):`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ check-private-leaks.test: ${passed} checks passed.`);
