// Third-party license bundle staleness gate (issue #226).
//
// THIRD-PARTY-LICENSES.md is generated from the two dependency lockfiles by
// scripts/gen-third-party-licenses.mjs, which embeds a sha256 of each lockfile
// AND of the bundle's own body in an HTML comment. This gate recomputes all
// three and fails on mismatch, so a dependency change cannot merge without
// regenerating the attribution bundle, and an accidental hand-edit to the
// bundle body is caught.
//
// Scope, stated plainly: this proves STALENESS and accidental-edit integrity,
// not authenticity. The hashes are plaintext, so anyone can forge a matching
// comment on a hand-written bundle - content correctness rests on PR review
// of the bundle diff (and on the generator's accepted-license allowlist,
// which runs at generation time, not here).
//
// A pure hash comparison on purpose: regeneration needs cargo-about installed
// and a populated node_modules, which CI should not pay for on every push.
// The hash pins the full lockfile, so a devDependency-only change also asks
// for a regen - that run is a no-op diff, and the alternative (hashing only
// the prod subset) would mean reimplementing pnpm's prod-graph resolution
// here just to save one local command.
//
// WHERE IT RUNS - the leak-scan workflow, NOT the `check` job: `check`
// path-ignores `**/*.md`, so a PR touching only THIRD-PARTY-LICENSES.md would
// never run a gate that lived there (the "gate consumes a path-ignored file"
// class). leak-scan has no path filter, needs no pnpm install, and already
// hosts the other whole-tree text guards.
//
// Usage:  node scripts/check-third-party-licenses.mjs   (or: pnpm lint:third-party)
// Regenerate:  pnpm gen:third-party
//   (needs: cargo install cargo-about --version 0.9.1 --locked --features cli)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { HASHED_INPUTS, hashNormalised } from './gen-third-party-licenses.mjs';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const BUNDLE = 'THIRD-PARTY-LICENSES.md';
// HASHED_INPUTS (the two lockfiles plus the generator and its cargo-about
// config, so a renderer or harvest change also demands a regen, not just a
// dependency bump) and the CRLF-normalised hash both come from the generator
// itself, so the list the checker verifies is the list the generator wrote.
// They used to be two copies held together by a "lockstep" comment (#576
// E18(c)). The generator has an isMain guard and only stdlib imports, so this
// import neither runs it nor needs a `pnpm install`.

// Sibling gates cap reads too; the real bundle is ~0.5 MB, so 32 MB is pure
// headroom. Fail-closed with its own message rather than mislabeling an
// unreadable giant as "missing".
export const MAX_BUNDLE_BYTES = 32 * 1024 * 1024;


// Returns a list of problem strings; empty means the bundle is current.
// `maxBytes` exists for the fixture test, which proves the cap with a few KB
// instead of writing a 33 MB file per run (#576 E18(b)).
export function checkBundle(root, { maxBytes = MAX_BUNDLE_BYTES } = {}) {
  const bundlePath = path.join(root, BUNDLE);
  let stat;
  try {
    stat = fs.statSync(bundlePath);
  } catch {
    return [`${BUNDLE} is missing`];
  }
  if (stat.size > maxBytes) {
    return [`${BUNDLE} is ${stat.size} bytes (> ${maxBytes} cap) - not a generated bundle`];
  }
  let body;
  try {
    // Normalised up front so the comment regex and the body hash both work
    // on an autocrlf (CRLF) checkout of the bundle itself.
    body = fs.readFileSync(bundlePath, 'utf8').replace(/\r\n/g, '\n');
  } catch {
    return [`${BUNDLE} is unreadable`];
  }
  const comment = body.match(/<!-- lockfile-hashes\n([\s\S]*?)-->\n/);
  if (!comment) return [`${BUNDLE} has no lockfile-hashes comment - regenerate it`];
  const recorded = new Map(
    [...comment[1].matchAll(/^(\S+)=([0-9a-f]{64})$/gm)].map((m) => [m[1], m[2]]),
  );
  const problems = [];
  for (const rel of HASHED_INPUTS) {
    const want = recorded.get(path.posix.basename(rel));
    if (!want) {
      problems.push(`${BUNDLE} records no hash for ${rel} - regenerate it`);
      continue;
    }
    const got = hashNormalised(fs.readFileSync(path.join(root, rel), 'utf8'));
    if (got !== want) problems.push(`${rel} changed since ${BUNDLE} was generated`);
  }
  // Everything after the comment's closing marker is the generated body; a
  // mismatch means the bundle was edited by hand (or truncated).
  const wantBody = recorded.get('bundle-body');
  if (!wantBody) {
    problems.push(`${BUNDLE} records no bundle-body hash - regenerate it`);
  } else {
    const tail = body.slice(body.indexOf(comment[0]) + comment[0].length);
    if (hashNormalised(tail) !== wantBody) {
      problems.push(`${BUNDLE} body does not match its recorded hash - it was edited by hand; regenerate it`);
    }
  }
  return problems;
}

function main() {
  const problems = checkBundle(REPO_ROOT);
  if (problems.length === 0) {
    console.log(`✓ ${BUNDLE} matches its generator inputs (lockfiles, generator, about.toml) and its own body`);
    return;
  }
  for (const p of problems) console.error(`✗ ${p}`);
  console.error(
    '\nRegenerate the third-party attribution bundle:\n' +
      '  pnpm gen:third-party\n' +
      '(needs cargo-about: cargo install cargo-about --locked --features cli)',
  );
  process.exit(1);
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) main();
