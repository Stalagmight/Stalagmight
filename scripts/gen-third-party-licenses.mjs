// Third-party license bundle generator (issue #226).
//
// Regenerates THIRD-PARTY-LICENSES.md at the repo root: every package that
// ships in the built app - the full Rust dependency graph (telemetry features
// included, matching what release.yml builds) plus the production npm
// dependencies vite bundles into the frontend - with each distinct license
// text reproduced verbatim. MIT/BSD/Apache all condition redistribution on
// retaining their notices, so this file is a legal requirement of shipping
// binaries, not documentation polish.
//
// Sources:
//   - Rust: `cargo about generate --format json` (config: src-tauri/about.toml,
//     whose `accepted` allowlist makes an unexpected license a hard failure).
//     First-party workspace crates are dropped here - the app's own AGPL text
//     lives in LICENSE, not in its third-party attributions.
//   - JS: `pnpm licenses list --prod --json`, then each package's shipped
//     LICENSE/NOTICE file read from node_modules. devDependencies are excluded
//     on purpose: they never reach the built app.
//
// The output embeds sha256 hashes of Cargo.lock, pnpm-lock.yaml, and its own
// body; scripts/check-third-party-licenses.mjs (leak-scan workflow) compares
// them so the committed bundle cannot silently drift from the dependency set
// or be hand-edited unnoticed. Regenerate with `pnpm gen:third-party` after
// any dependency change (needs cargo-about, pinned so harvested texts do not
// churn across its releases:
// `cargo install cargo-about --version 0.9.1 --locked --features cli`).
//
// Deterministic on purpose: same lockfiles -> byte-identical output.
// Caveat the hashes cannot see: the JS half reads license files from the
// INSTALLED node_modules, so regenerate only after a fresh
// `pnpm install --frozen-lockfile` - a drifted install stamps correct hashes
// over content from the wrong tree.

import { execFileSync } from 'child_process';
import { createHash } from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const OUT_FILE = path.join(REPO_ROOT, 'THIRD-PARTY-LICENSES.md');

// Workspace crates whose license is the app's own, not a third-party notice.
// Names every planned member of the crate split up front so each extraction
// commit only has to add the member, not remember this list.
const FIRST_PARTY_CRATES = new Set([
  'stalagmight',
  'stalagmight-models',
  'stalagmight-telemetry',
  'stalagmight-parsers',
  'stalagmight-persistence',
  'stalagmight-export',
  'stalagmight-speleodb',
  'stalagmight-tiles',
  'stalagmight-core',
]);

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');
// Hashes are CRLF-normalised on both the generator and checker side, so a
// core.autocrlf=true checkout (Windows) regenerates the same hashes CI's LF
// checkout verifies. (A .gitattributes eol pin was tried instead and broke
// test-release.sh's CRLF-tolerance case - release.sh must keep accepting a
// CRLF Cargo.lock, so the gate normalises rather than the checkout.)
// Exported: scripts/check-third-party-licenses.mjs imports this and
// HASHED_INPUTS instead of keeping lockstep copies (#576 E18(c)).
export const hashNormalised = (text) => sha256(text.replace(/\r\n/g, '\n'));
const inputHash = (rel) => hashNormalised(fs.readFileSync(path.join(REPO_ROOT, rel), 'utf8'));

// Everything the output is a pure function of (given a pinned cargo-about and
// a fresh install): the two lockfiles, this generator, and the cargo-about
// config. Hashing the last two means a renderer or harvest-config change also
// demands a regen - the checker imports this very list, so the two cannot drift.
export const HASHED_INPUTS = [
  'src-tauri/Cargo.lock',
  'pnpm-lock.yaml',
  'scripts/gen-third-party-licenses.mjs',
  'src-tauri/about.toml',
];

// CRLF-normalise but otherwise keep license text verbatim - the notices are
// the payload here.
const normalise = (text) => text.replace(/\r\n/g, '\n').replace(/\s+$/, '') + '\n';

function run(cmd, args, opts = {}) {
  // On Windows pnpm is pnpm.cmd, and post-CVE-2024-27980 Node refuses to
  // execFileSync a .cmd without a shell. Args here are static flags plus
  // repo-root-derived paths, so shell interpretation is safe.
  const win = process.platform === 'win32' && cmd === 'pnpm';
  return execFileSync(win ? 'pnpm.cmd' : cmd, args, {
    cwd: REPO_ROOT,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    shell: win,
    ...opts,
  });
}

// ---------------------------------------------------------------------------
// Rust: cargo-about. One entry per distinct harvested license text, each with
// the crates it covers.
function collectRust() {
  let json;
  try {
    json = run('cargo', [
      'about', 'generate',
      '--manifest-path', path.join(REPO_ROOT, 'src-tauri', 'Cargo.toml'),
      '--features', 'telemetry',
      '--format', 'json',
    ]);
  } catch (e) {
    const hint = /not found|no such command/i.test(String(e.message))
      ? 'cargo-about is not installed. Install it with: cargo install cargo-about --version 0.9.1 --locked --features cli'
      : String(e.stderr || e.message).slice(0, 4000);
    throw new Error(`cargo about generate failed.\n${hint}`);
  }
  const { licenses } = JSON.parse(json);
  if (!Array.isArray(licenses) || licenses.length === 0) {
    throw new Error('cargo about generate returned no licenses - refusing to write an empty bundle');
  }
  const groups = [];
  for (const lic of licenses) {
    const packages = (lic.used_by ?? [])
      .map((u) => ({ name: u.crate.name, version: u.crate.version }))
      .filter((p) => !FIRST_PARTY_CRATES.has(p.name));
    if (packages.length === 0) continue;
    groups.push({
      licenseId: lic.id,
      licenseName: lic.name,
      text: normalise(lic.text),
      packages,
    });
  }
  return groups;
}

// ---------------------------------------------------------------------------
// JS: pnpm's production dependency closure, license texts read from the
// packages' own shipped files.
const LICENSE_FILE_RE = /^(licen[cs]e|copying|notice)([-._].*)?(\.(md|txt|markdown))?$/i;

// Packages whose published npm tarball ships NO license file. Each entry was
// vetted by hand: the text is the upstream repository's own notice, reproduced
// verbatim, with the source recorded. Generation FAILS on a license-file-less
// package that is not listed here - shipping an attribution bundle with no
// actual terms for a package is exactly what this file exists to prevent, so
// a new hit means vetting the upstream and adding an entry, not a warning.
const VETTED_MISSING_LICENSE = new Map(Object.entries({
  'html-parse-stringify@3.0.1': {
    licenseId: 'MIT',
    source: 'https://github.com/henrikjoreteg/html-parse-stringify/blob/master/LICENSE (verified 2026-08-03)',
    text: `MIT License

Copyright (c) 2025 Henrik Joreteg <henrik@joreteg.com>

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
`,
  },
  'murmurhash-js@1.0.0': {
    licenseId: 'MIT',
    source: 'https://github.com/mikolalysenko/murmurhash-js/blob/master/README.md ("License (MIT)" section, verified 2026-08-03)',
    text: `Copyright (c) 2011 Gary Court

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
`,
  },
  // No license field in its package.json either (pnpm reports "Unknown"), but
  // upstream states MIT: the fork's README carries the original jsonlint
  // notice verbatim.
  '@mapbox/jsonlint-lines-primitives@2.0.2': {
    licenseId: 'MIT',
    source: 'https://github.com/mapbox/jsonlint/blob/master/README.md ("MIT License" section, verified 2026-08-03; fork of zaach/jsonlint, same notice)',
    text: `Copyright (C) 2012 Zachary Carter

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
`,
  },
}));

function licenseFilesIn(dir) {
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return [];
  }
  return entries
    .filter((e) => e.isFile() && LICENSE_FILE_RE.test(e.name))
    .map((e) => e.name)
    .sort();
}

function collectJs() {
  const parsed = JSON.parse(run('pnpm', ['licenses', 'list', '--prod', '--json']));
  if (parsed.error) throw new Error(`pnpm licenses failed: ${parsed.error.message}`);
  const groups = [];
  const unvetted = [];
  for (const [licenseId, pkgs] of Object.entries(parsed)) {
    for (const pkg of pkgs) {
      // pnpm reports one entry per package; versions[] and paths[] are
      // parallel arrays (one installed directory per resolved version), so
      // the label for each path is the version at the same index.
      for (const [i, dir] of (pkg.paths ?? []).entries()) {
        const files = licenseFilesIn(dir);
        const label = { name: pkg.name, version: pkg.versions[i] ?? pkg.versions[0] };
        if (files.length === 0) {
          const vetted = VETTED_MISSING_LICENSE.get(`${label.name}@${label.version}`);
          if (!vetted) {
            unvetted.push(`${label.name}@${label.version} (declared: ${licenseId})`);
            continue;
          }
          groups.push({
            licenseId: vetted.licenseId,
            licenseName: vetted.licenseId,
            note: `The published package ships no license file; notice reproduced from ${vetted.source}.`,
            text: normalise(vetted.text),
            packages: [label],
          });
          continue;
        }
        const text = files
          .map((f) => {
            const body = normalise(fs.readFileSync(path.join(dir, f), 'utf8'));
            return files.length > 1 ? `--- ${f} ---\n\n${body}` : body;
          })
          .join('\n');
        groups.push({ licenseId, licenseName: licenseId, text, packages: [label] });
      }
    }
  }
  if (unvetted.length > 0) {
    throw new Error(
      'refusing to generate: these npm packages ship no license file and have no vetted entry in VETTED_MISSING_LICENSE:\n' +
        unvetted.map((u) => `  - ${u}`).join('\n') +
        '\nVet each upstream notice by hand and add an entry (with source and date) before regenerating.',
    );
  }
  return groups;
}

// ---------------------------------------------------------------------------
// Merge packages that carry byte-identical license text into one block, then
// order everything deterministically.
function dedupe(groups) {
  const byText = new Map();
  for (const g of groups) {
    const key = `${g.licenseId}\n${sha256(g.text)}`;
    const existing = byText.get(key);
    if (existing) existing.packages.push(...g.packages);
    else byText.set(key, { ...g, packages: [...g.packages] });
  }
  const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
  const out = [...byText.values()];
  for (const g of out) {
    g.packages.sort((a, b) => cmp(a.name, b.name) || cmp(a.version, b.version));
    g.packages = g.packages.filter(
      (p, i) => i === 0 || p.name !== g.packages[i - 1].name || p.version !== g.packages[i - 1].version,
    );
  }
  // The text-hash tiebreak keeps the order fully determined even when two
  // groups share a license id and lead package (e.g. two versions of one
  // package with differing license texts) - without it the tie falls to
  // pnpm's JSON iteration order.
  out.sort(
    (a, b) =>
      cmp(a.licenseId, b.licenseId) ||
      cmp(a.packages[0].name, b.packages[0].name) ||
      cmp(sha256(a.text), sha256(b.text)),
  );
  return out;
}

function renderSection(title, intro, groups) {
  const parts = [`## ${title}\n`, `${intro}\n`];
  for (const g of groups) {
    const pkgList = g.packages.map((p) => `\`${p.name} ${p.version}\``).join(', ');
    parts.push(`### ${g.licenseName}\n`);
    parts.push(`Applies to: ${pkgList}\n`);
    if (g.note) parts.push(`${g.note}\n`);
    // Four-backtick fences: a hostile npm package's license file containing a
    // ``` line must not be able to break out of the block and inject markdown
    // into a file the em-dash gate exempts.
    parts.push('````text\n' + g.text + '````\n');
  }
  return parts.join('\n');
}

function main() {
  const rust = dedupe(collectRust());
  const js = dedupe(collectJs());
  const rustCount = new Set(rust.flatMap((g) => g.packages.map((p) => `${p.name}@${p.version}`))).size;
  const jsCount = new Set(js.flatMap((g) => g.packages.map((p) => `${p.name}@${p.version}`))).size;

  const body = [
    renderSection(
      'npm packages',
      'The production JavaScript dependencies bundled into the frontend, with the license/notice files each package ships.',
      js,
    ),
    renderSection(
      'Rust crates',
      'The dependency graph of the release binary (telemetry features included, dev-dependencies excluded), as harvested by cargo-about.',
      rust,
    ),
  ].join('\n');

  // The body hash covers every byte after the comment's closing marker, so
  // the checker can recompute it without re-deriving any structure.
  const tail = '\n' + body;
  const header = `# Third-party licenses

Stalagmight's shipped binaries include the open-source packages listed below:
${rustCount} Rust crates and ${jsCount} npm packages. Each distinct license
text is reproduced verbatim, as those licenses require. Stalagmight itself is
licensed AGPL-3.0-or-later; see [LICENSE](LICENSE).

Generated by \`pnpm gen:third-party\` (scripts/gen-third-party-licenses.mjs)
from the dependency lockfiles - do not edit by hand. CI fails if this file is
stale or hand-edited; regenerate after any dependency change.

<!-- lockfile-hashes
${HASHED_INPUTS.map((rel) => `${path.posix.basename(rel)}=${inputHash(rel)}`).join('\n')}
bundle-body=${hashNormalised(tail)}
-->
`;

  fs.writeFileSync(OUT_FILE, header + tail);
  const size = (fs.statSync(OUT_FILE).size / 1024).toFixed(0);
  console.log(`wrote THIRD-PARTY-LICENSES.md: ${rustCount} crates + ${jsCount} npm packages, ${rust.length + js.length} license blocks, ${size} KB`);
}

// Only when run as a script: importing this module (the checker does, for
// HASHED_INPUTS) must never regenerate - or even try to regenerate - the
// shipped bundle (#576 E18(c)).
const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) main();
