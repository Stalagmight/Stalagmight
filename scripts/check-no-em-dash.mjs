// Unicode-dash guard for the tracked tree (issue #293).
//
// CLAUDE.md has said "No em-dashes" for a long time, but nothing enforced it,
// so the tree accumulated ~3.7k of them and review bots re-flagged pre-existing
// ones on nearly every PR. PR #293 swept them all to ASCII hyphens; this guard
// keeps them from creeping back.
//
// WHAT IT FLAGS - nineteen dash-like code points, all replaced by a plain
// ASCII hyphen:
//   U+2010 hyphen, U+2011 non-breaking hyphen, U+2012 figure dash,
//   U+2013 en dash, U+2014 em dash, U+2015 horizontal bar, U+2212 minus sign,
//   U+00AD soft hyphen, U+FE58 small em dash, U+FE63 small hyphen-minus,
//   U+FF0D fullwidth hyphen-minus, U+2E3A two-em dash, U+2E3B three-em dash,
//   U+FE31 / U+FE32 vertical em / en dash, U+2E40 double hyphen, U+2E17
//   double oblique hyphen (those four since #576 RV3-FE-NITS), U+058A
//   Armenian hyphen, U+2E5D oblique hyphen (#576 RV4-FE-FEB-11).
// The last six were added in #576 (E18(d)). U+00AD is the sharpest of them:
// inside a word it renders as nothing at all, so `pro<U+00AD>ject` and
// `project` look identical in review while comparing unequal (a test ID, a
// store key). The other five are the compatibility/small-form lookalikes of
// the hyphen and em dash. All had zero hits in the tree when they were added.
// The en dash is included deliberately (issue #293 q6-B): it IS the correct
// glyph for a numeric range, but a strictly-ASCII tree is the house rule and a
// context-sensitive "ranges are fine" exception would be hard to test and to
// explain. `0-360` and `Gauss-Newton` are the accepted cost.
// U+2010/U+2011 are included because they are visually INDISTINGUISHABLE from
// the ASCII hyphen: a reviewer cannot see one, so only a scanner can. The sweep
// missed a U+2010 in src-tauri/src/export/tml.rs while editing two lines away,
// which is exactly the failure mode that argues for banning them.
//
// WHAT IT DOES NOT FLAG - box-drawing characters (U+2500..U+257F) and every
// other non-dash glyph. ASCII diagrams in docs/ rely on those for alignment and
// substituting them would wreck the drawing, so they stay.
//
// WHERE IT RUNS - the leak-scan workflow (.github/workflows/leak-scan.yml),
// which has NO path filter. This is load-bearing and mirrors the placement of
// scripts/check-private-leaks.mjs: the `check` and `smoke` jobs BOTH path-ignore
// `**/*.md` and `docs/**`, which is exactly where the prose (and most of the
// dashes) live, so a gate wired into either would be a no-op for the files that
// carry the most risk.
//
// This file itself contains no BANNED code point - they are built with
// String.fromCharCode rather than written literally - so the
// scanner never has to exclude itself, and neither does its test.
//
// Issue #293 settled nearly every edge case with a substitution rather than an
// exception (i18n display strings, the UI N/A glyphs, the glued `-ref-` diagram
// form), so the ALLOW table below stays as small as possible. It exists for the
// one case a substitution cannot serve: prose that has to SHOW the banned glyph
// to state the rule. CLAUDE.md is that case - the file defining "no em-dashes"
// is the file that must be able to print one.
//
// ALLOW is budgeted, not blanket: an entry permits at most `max` occurrences of
// ONE codepoint in ONE file. The 51st dash in CLAUDE.md still fails, so the file
// is not carved out of the scan - only its documented example is.
//
// Whole PATHS are also excluded further below (vendored, generated, lockfiles,
// binaries), so the guard is not unconditional. Prefer an ALLOW row with a
// written reason over widening those prefixes. Those exclusions cover CONTENT
// only: every tracked path string is scanned too, excluded ones included, and
// a path hit is never budgeted by ALLOW (#576 E18(e)).
//
// Usage:  node scripts/check-no-em-dash.mjs   (or: pnpm lint:no-em-dash)

import * as path from 'path';
import { fileURLToPath } from 'url';
import {
  MAX_SCAN_BYTES, hasBinaryExt, looksBinary, listTracked, makeReader, asEntry, toScanText,
} from './lib/tracked-files.mjs';

// Re-exported for the fixture test (the helper now lives in lib/).
export { looksBinary };

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// One entry per banned code point: {char, name}. Written as escapes so this
// source file carries no banned code point and does not trip its own scan.
// (It is not pure ASCII: the pass/fail lines print U+2713/U+2717, as the sibling
// guards do. Only the banned set has to be absent for self-scanning to work.)
export const BANNED = [
  { code: 0x2010, name: 'U+2010 hyphen' },
  { code: 0x2011, name: 'U+2011 non-breaking hyphen' },
  { code: 0x2012, name: 'U+2012 figure dash' },
  { code: 0x2013, name: 'U+2013 en dash' },
  { code: 0x2014, name: 'U+2014 em dash' },
  { code: 0x2015, name: 'U+2015 horizontal bar' },
  { code: 0x2212, name: 'U+2212 minus sign' },
  { code: 0x00ad, name: 'U+00AD soft hyphen' },
  { code: 0xfe58, name: 'U+FE58 small em dash' },
  { code: 0xfe63, name: 'U+FE63 small hyphen-minus' },
  { code: 0xff0d, name: 'U+FF0D fullwidth hyphen-minus' },
  { code: 0x2e3a, name: 'U+2E3A two-em dash' },
  { code: 0x2e3b, name: 'U+2E3B three-em dash' },
  // Further dash lookalikes (#576 RV3-FE-NITS): the vertical presentation
  // forms render as a plain dash in a horizontal run, and the double hyphens
  // as two. U+301C wave dash stays allowed: a tilde-like CJK range mark.
  { code: 0xfe31, name: 'U+FE31 vertical em dash' },
  { code: 0xfe32, name: 'U+FE32 vertical en dash' },
  { code: 0x2e40, name: 'U+2E40 double hyphen' },
  { code: 0x2e17, name: 'U+2E17 double oblique hyphen' },
  // The two remaining Pd members that render as a plain dash in Latin text
  // (#576 RV4-FE-FEB-11). Other Pd letters (U+05BE, U+1400, U+1806, U+2E1A,
  // U+30A0, U+3030) look nothing like an ASCII hyphen there.
  { code: 0x058a, name: 'U+058A Armenian hyphen' },
  { code: 0x2e5d, name: 'U+2E5D oblique hyphen' },
].map((b) => ({ ...b, char: String.fromCharCode(b.code) }));
const BANNED_NAME = new Map(BANNED.map((b) => [b.char, b.name]));
const BANNED_RE = new RegExp(`[${BANNED.map((b) => b.char).join('')}]`, 'g');

// MAX_SCAN_BYTES (5 MB) comes from scripts/lib/tracked-files.mjs, shared with
// the leak guard. An oversized file FAILS the gate (see below).

// Whole files excluded from the scan.
const EXCLUDE_EXACT = new Set([
  'src/types/ipc.generated.ts', // generated IPC types (regenerate, don't hand-edit)
  // Generated third-party license texts, reproduced verbatim (same class as the
  // vendored crate below) - upstream notices legitimately carry Unicode dashes.
  'THIRD-PARTY-LICENSES.md',
]);
const EXCLUDE_PREFIXES = [
  'src-tauri/vendor/', // vendored upstream crate - left verbatim (its own author/license)
  'src/types/ipc/', // generated IPC types
];
// Extensions skipped before reading come from BINARY_EXT in
// scripts/lib/tracked-files.mjs (shared with the leak guard). Text formats that
// can legitimately carry a dash - `.svg`, `.snap`, `.json` - are deliberately
// NOT in it, so they get scanned.
//
// `dmp`, `tml` and `stalx` are survey-data formats, not opaque binaries: `.dmp`
// in particular is plain ASCII (the golden parser fixtures under
// e2e-playwright/fixtures/ are readable text). They are skipped anyway because
// they are DATA, where a byte is load-bearing - a dash inside a fixture is the
// input under test, and "fix" would mean corrupting the case it pins. That is a
// deliberate carve-out, not an oversight, and the fixture test pins it.
//
// A tracked SYMLINK is never excluded by extension: its content is its target
// string, which is text whatever the name says.
export function isExcluded(rel, { symlink = false } = {}) {
  if (EXCLUDE_EXACT.has(rel)) return true;
  if (EXCLUDE_PREFIXES.some((p) => rel.startsWith(p))) return true;
  if (rel.endsWith('Cargo.lock') || rel.endsWith('pnpm-lock.yaml')) return true;
  return !symlink && hasBinaryExt(rel);
}

// Scan one file's text (or one path string). Returns [{lineNo, col, char, name}], one row per banned
// code point. Reports the offending CHARACTER and its position only - never the
// surrounding line - so the CI log stays terse on a bulk regression.
export function findDashes(text) {
  const out = [];
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i++) {
    BANNED_RE.lastIndex = 0;
    let m;
    while ((m = BANNED_RE.exec(lines[i])) !== null) {
      out.push({ lineNo: i + 1, col: m.index + 1, char: m[0], name: BANNED_NAME.get(m[0]) });
    }
  }
  return out;
}

// The gate fails if any banned dash was found OR any tracked file could not be
// scanned. A skip (oversized/unreadable) is a FAILURE, not a warning, matching
// check-private-leaks.mjs: a file the scanner declines to read must not pass as
// clean, or the tree can drift back behind it.
export function gateFails(violations, skipped) {
  return violations.length > 0 || skipped.length > 0;
}

// Render a repo-relative path for printing into a PUBLIC CI log.
//
// This runs on `pull_request`, so on a fork PR every path comes from an
// attacker-chosen tree. Git permits a newline and `::` inside a tree entry name,
// and GitHub Actions parses `::workflow-command::` at the start of a log line -
// so a file named with an embedded newline plus `::error::` could forge
// annotations in this job's log. JSON-quoting escapes the control characters and
// makes the path unambiguous; the leading `::` of any forged command then sits
// mid-string rather than at line start.
export function safePath(rel) {
  return JSON.stringify(rel);
}

// Budgeted exceptions: {file, code, max, why}. Each row permits at most `max`
// occurrences of that ONE codepoint in that ONE file; anything beyond the budget
// is reported as a violation, so the file stays scanned. Keep this table short.
export const ALLOW = [
  {
    file: 'CLAUDE.md',
    code: 0x2014,
    max: 1,
    why: 'the rule statement itself shows the glyph it bans ("instead of \'<em-dash>\' use \'-\'"), '
      + 'which is clearer to a reader than naming a codepoint. The file cannot follow its own rule here.',
  },
];

// Drop allowed violations, keeping any that exceed a row's budget. Counting is
// per (file, codepoint) so an allowance for one dash never licenses another.
export function applyAllow(violations, allow = ALLOW) {
  const budget = new Map(allow.map((a) => [`${a.file}\0${a.code}`, a.max]));
  const kept = [];
  for (const v of violations) {
    const key = `${v.rel}\0${v.char.charCodeAt(0)}`;
    const left = budget.get(key);
    if (left > 0) {
      budget.set(key, left - 1);
      continue;
    }
    kept.push(v);
  }
  return kept;
}

// Cap on printed rows, per category. A reverted sweep (violations) or a tree of
// planted NUL files (skips) would otherwise flood the CI log; the totals are
// still reported truthfully in the "... and N more" tail.
const SHOWN = 50;

// Scan tracked entries: repo-relative paths, or {rel, symlink} as
// listTracked() returns them. `readFile` is injected so the fixture test can
// drive every branch - the size cap and the unreadable path are otherwise
// unreachable on a healthy tree, and an untested branch in a fail-closed gate is
// how a gate silently starts passing everything. It returns a file's bytes, or
// `{ symlink: '<target>' }` for a link (see scripts/lib/tracked-files.mjs).
export function scan(files, readFile) {
  const violations = [];
  const skipped = []; // {rel, reason} - each FAILS the gate (fail-closed)
  const symlinks = []; // scanned as their link text, never followed
  let scanned = 0;
  const pathViolations = [];
  const entries = [...files].map(asEntry).sort((a, b) => (a.rel < b.rel ? -1 : a.rel > b.rel ? 1 : 0));
  for (const { rel, symlink } of entries) {
    // The PATH is published too (#576 E18(e)): scan every tracked path string,
    // BEFORE the content exclusions - a dash in the name of a vendored file, a
    // lockfile or an image is just as visible in the tree as one in a doc.
    for (const v of findDashes(rel)) pathViolations.push({ rel, path: true, ...v });
    if (isExcluded(rel, { symlink })) continue;
    let result;
    try {
      result = readFile(rel);
    } catch (e) {
      // staged-but-absent / submodule pointer / permission / race.
      skipped.push({ rel, reason: `unreadable (${e.code || e.message})` });
      continue;
    }
    const t = toScanText(result, MAX_SCAN_BYTES);
    if (t.skip) {
      skipped.push({ rel, reason: t.skip });
      continue;
    }
    // A symlink is scanned as its TARGET STRING - what git stores and publishes
    // for it - and never dereferenced: following it would let a fork PR point
    // the scanner outside the repo, and skipping it (the old behaviour) let a
    // banned dash ride in the link text. AGENTS.md -> CLAUDE.md scans "CLAUDE.md".
    if (t.link) symlinks.push(rel);
    scanned++;
    for (const v of findDashes(t.text)) violations.push({ rel, link: t.link, ...v });
  }
  // ALLOW budgets content only; a path hit is never allowlisted.
  return { violations: [...pathViolations, ...applyAllow(violations)], skipped, scanned, symlinks };
}

export function report(result, log = console.log, err = console.error) {
  const { violations, skipped, scanned, symlinks = [] } = result;
  if (!gateFails(violations, skipped)) {
    const note = symlinks.length ? ` (${symlinks.length} symlink(s) scanned as their link text, not followed)` : '';
    log(`✓ no-em-dash scan: ${scanned} tracked text files, no Unicode dashes.${note}`);
    return 0;
  }

  err('\n✗ no-em-dash scan failed:\n');
  for (const v of violations.slice(0, SHOWN)) {
    const where = v.path ? ` (path):${v.col}` : `${v.link ? ' (symlink target)' : ''}:${v.lineNo}:${v.col}`;
    err(`  ${safePath(v.rel)}${where}  ${v.name}`);
  }
  if (violations.length > SHOWN) {
    err(`  ... and ${violations.length - SHOWN} more (${violations.length} total)`);
  }
  for (const s of skipped.slice(0, SHOWN)) {
    err(`  [unscanned] ${safePath(s.rel)} - ${s.reason}`);
    err('      could not scan this file; failing closed (shrink it, or exclude it deliberately).');
  }
  if (skipped.length > SHOWN) {
    err(`  ... and ${skipped.length - SHOWN} more unscanned (${skipped.length} total)`);
  }
  err(
    '\nReplace each one with an ASCII hyphen ("-"); delete a U+00AD soft hyphen outright.\n' +
      'CLAUDE.md: "No em-dashes".\n' +
      'See the header of scripts/check-no-em-dash.mjs for what is banned and why\n' +
      '(box-drawing characters in ASCII diagrams are NOT banned).\n',
  );
  return 1;
}

export function main() {
  let files;
  try {
    files = listTracked(REPO_ROOT);
  } catch (e) {
    // The first line only (`Command failed: ...`): the rest repeats the child's
    // stderr, which already reached the terminal and can name the checkout's
    // absolute path (#576 W51).
    console.error(`check-no-em-dash: could not list tracked files (run from a git repo): ${e.message.split('\n')[0]}`);
    return 1;
  }
  return report(scan(files, makeReader(REPO_ROOT)));
}

const isMainModule =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMainModule) process.exit(main());
