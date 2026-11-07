// File-size ratchet for the TypeScript trees: the frontend (src/**/*.{ts,tsx})
// and, since #576 (E8-D2), the two test-automation trees e2e-playwright/ and e2e/.
//
// Port of scripts/check-rust-file-size.mjs (#157) to TypeScript/TSX (#275). Same
// motivation: several components (DmpNewImportDialog, SurveyLayer, MapCanvas)
// grew to thousands of lines, which raises agent-context cost and hurts
// readability; this guard stops the rest of the tree growing back the same way.
//
// Metric: PRODUCTION code LOC = source lines minus blank/comment lines (line
// comments, block comments, and comment-shaped content masked out of strings/
// templates so it can't be mistaken for a real comment). Unlike the Rust guard,
// there is no `#[cfg(test)]`-style in-file test-module stripping - TS tests live
// in wholly separate files, which are excluded from the scan entirely.
//
// Policy (baseline = ts-file-size-baseline.json), identical to the Rust ratchet:
//   - A file at/above `WARN_LOC` (1500) prod LOC is "large" and FROZEN by the
//     ratchet: it may shrink, never grow past its recorded baseline. Split it at
//     a component/module boundary, or run `--update` to rebaseline an
//     intentional change.
//   - A NEW (non-baselined) file may not be introduced at/above `WARN_LOC` - no
//     new god-files born just under the cap.
//   - A NEW or GROWN file may never exceed the hard `FAIL_LOC` (2000) budget.
//   - Files below `WARN_LOC` grow freely (so ordinary edits to small files don't
//     need a rebaseline); grandfathered files already over the cap only need to
//     stop growing.
// Anything at/above `WARN_LOC` that isn't a violation prints a non-fatal warning.
//
// Scope: filesystem-walked via the shared scripts/lib/src-files.mjs lister (not
// `git ls-files` - an untracked file must still be caught):
//   - `src/**/*.{ts,tsx}`, excluding its one test/generated predicate pair:
//     test files (`__tests__/`, `*.test.*`, `*.spec.*`, `src/test/**`) and the
//     generated IPC types (`src/types/ipc/**`, `src/types/ipc.generated.ts`,
//     ts-rs output - see CLAUDE.md "IPC Types Are Generated");
//   - every `.ts`/`.tsx` under E2E_ROOTS (`e2e-playwright/`, `e2e/`) - specs,
//     helpers and the IPC mock included, since there the test code IS the code
//     (the mock alone is ~4.8k LOC) - except the generated replay specs under
//     `e2e/agent/replays/` (`pnpm agent:replay:gen` output).
// The same thresholds apply everywhere; today's large e2e files are baselined
// and frozen like any other. A symlink in scope is never followed and FAILS the
// gate (it cannot be measured without reading something the gate did not walk).
//
// Runs in BOTH the `check` job (which sees src/ and e2e/ changes) and the
// `smoke` job (which sees e2e-playwright/ changes - `check` path-ignores them).
//
// The strip heuristic is pinned by scripts/check-ts-file-size.test.mjs.
//
// Usage:  node scripts/check-ts-file-size.mjs            (pnpm lint:ts-file-size)
//         node scripts/check-ts-file-size.mjs --update    (regenerate the baseline)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { listSrcFiles, walkFiles, symlinkProblem } from './lib/src-files.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SRC_DIR = path.join(REPO_ROOT, 'src');

// The test-automation trees (#576 E8-D2), and the one generated part of them.
export const E2E_ROOTS = ['e2e-playwright', 'e2e'];
export function isGeneratedE2e(rel) {
  return rel.startsWith('e2e/agent/replays/');
}

// Every in-scope file as {files, symlinks}, repo-relative. A missing e2e root
// is tolerated (a partial checkout); src/ is required (main() checks it).
export function listScopedFiles(root = REPO_ROOT) {
  const files = [];
  const symlinks = [];
  const add = (r) => {
    files.push(...r.files);
    symlinks.push(...r.symlinks);
  };
  add(listSrcFiles(root));
  for (const dir of E2E_ROOTS) {
    if (fs.existsSync(path.join(root, dir))) add(walkFiles(root, dir, { exts: ['.ts', '.tsx'], skip: isGeneratedE2e }));
  }
  return { files, symlinks };
}
const BASELINE_PATH = path.join(REPO_ROOT, 'ts-file-size-baseline.json');

// Thresholds are owned by the script (the single source of truth). The baseline
// JSON echoes them for readability, but `main()` asserts the echo matches these
// and `--update` always writes these - so editing the generated artifact can't
// quietly loosen the *global thresholds*. (A per-file baseline number can still be
// bumped to grant a specific file room, but that shows up in the baseline diff
// right next to the grown file, so it's review-visible, not silent.)
export const WARN_LOC = 1500;
export const FAIL_LOC = 2000;

// Files allowed to exceed the budget without ratcheting, each with a written
// reason. The baseline already grandfathers today's large files, so this is only
// for a future file that is genuinely irreducible. Add a row with a justification.
const ALLOW = [];

const MASK = 'M'; // non-whitespace placeholder for string/template content
function maskStr(s) {
  return s.replace(/\S/g, MASK); // keep whitespace (so an all-space string line stays blank)
}

// Characters after which a bare `/` is heuristically a regex literal start
// rather than division (the classic lexer disambiguation set: operators,
// punctuation that can't end an expression, and start-of-line/file). This is a
// best-effort heuristic (a full JS/TS lexer would track keyword context too,
// e.g. `return /x/`), not a parser - a missed regex just falls through to being
// treated as ordinary code characters, which is the SAFE direction (the
// dangerous direction is a regex body containing `//` or `/*` being missed as
// a comment-eater; that only bites if the regex is detected as division AND
// happens to contain those exact sequences - audited absent in the current tree).
const REGEX_PREV_CHARS = new Set(['(', '[', '{', ',', ';', ':', '=', '&', '|', '!', '?', '+', '-', '*', '%', '^', '~', '<', '>']);

// Scan a same-line regex literal body starting at `start` (line[start] === '/').
// Returns the exclusive end index (after any trailing flags), or -1 if no
// unescaped closing `/` is found on this line (regex literals cannot span
// lines in JS/TS) - the caller then falls back to treating `/` as an ordinary
// character rather than risking an unbounded consume.
function scanRegexEnd(line, start) {
  const n = line.length;
  let i = start + 1;
  let inClass = false;
  while (i < n) {
    const ch = line[i];
    if (ch === '\\') { i += 2; continue; }
    if (ch === '[') { inClass = true; i++; continue; }
    if (ch === ']') { inClass = false; i++; continue; }
    if (ch === '/' && !inClass) {
      let j = i + 1;
      while (j < n && /[a-zA-Z]/.test(line[j])) j++;
      return j;
    }
    i++;
  }
  return -1;
}

// Mask string-literal, template-literal, and comment spans so a comment-shaped
// sequence inside a string (e.g. `"http://example.com"`) can't be mistaken for
// a real comment, while keeping the LOC count faithful. Comments -> spaces (a
// comment-only line becomes blank, so it isn't counted). String/template
// content -> a non-whitespace placeholder `M` (so a multi-line template body
// still counts as code, like tokei; its content can't perturb later scanning).
//
// Handles: `//` line comments, `/* */` block comments (JS/TS don't nest them),
// single/double-quoted strings (with `\` escapes), template literals
// (`` ` `` ... `` ` ``, with nested `${ … }` expressions that can themselves
// contain strings/templates/comments/braces, tracked via a context stack), and
// a best-effort regex-literal skip (see scanRegexEnd).
export function maskNonCode(lines, warnings = []) {
  const masked = [];
  const stack = [{ type: 'code' }];
  let prevToken; // last significant (non-space) code char, for the regex/division heuristic
  for (const line of lines) {
    const n = line.length;
    let out = '';
    let i = 0;
    while (i < n) {
      const top = stack[stack.length - 1];
      const c = line[i];
      const c2 = line[i + 1];

      if (top.type === 'block') {
        if (c === '*' && c2 === '/') { stack.pop(); out += '  '; i += 2; continue; }
        out += ' '; i++; continue;
      }
      if (top.type === 'str') {
        if (c === '\\') { out += maskStr(line.slice(i, i + 2)); i += 2; continue; }
        if (c === top.quote) { stack.pop(); out += MASK; i++; prevToken = top.quote; continue; }
        out += maskStr(c); i++; continue;
      }
      if (top.type === 'template') {
        if (c === '\\') { out += maskStr(line.slice(i, i + 2)); i += 2; continue; }
        if (c === '`') { stack.pop(); out += MASK; i++; prevToken = '`'; continue; }
        if (c === '$' && c2 === '{') { stack.push({ type: 'expr', depth: 0 }); out += MASK + MASK; i += 2; continue; }
        out += maskStr(c); i++; continue;
      }

      // top.type is 'code' or 'expr' - both are code-like contexts that can open
      // nested strings/templates/comments/regexes.
      //
      // KNOWN LIMITATION: this masker has no distinct JSX-text context, so a
      // literal `//` inside raw JSX text (e.g. `<div>a // not b</div>`) is
      // treated as a line comment and blanks the rest of that line - an
      // under-count, not an over-count. Pinned as current behavior by the
      // "REGRESSION-KNOWN-LIMITATION" test in check-ts-file-size.test.mjs so a
      // future masker change surfaces it consciously rather than silently.
      if (c === '/' && c2 === '/') { out += ' '.repeat(n - i); i = n; break; }
      if (c === '/' && c2 === '*') { stack.push({ type: 'block' }); out += '  '; i += 2; continue; }
      if (c === '"' || c === "'") { stack.push({ type: 'str', quote: c }); out += MASK; i++; continue; }
      if (c === '`') { stack.push({ type: 'template' }); out += MASK; i++; continue; }
      if (c === '/' && (prevToken === undefined || REGEX_PREV_CHARS.has(prevToken))) {
        const end = scanRegexEnd(line, i);
        if (end !== -1) { out += maskStr(line.slice(i, end)); i = end; prevToken = '/'; continue; }
      }
      if (top.type === 'expr' && c === '{') { top.depth++; out += c; i++; prevToken = c; continue; }
      if (top.type === 'expr' && c === '}') {
        if (top.depth > 0) { top.depth--; out += c; i++; prevToken = c; continue; }
        stack.pop(); out += MASK; i++; prevToken = '}'; continue; // closes the `${ … }`, back to 'template'
      }
      out += c; i++;
      if (!/\s/.test(c)) prevToken = c;
    }
    masked.push(out);
  }
  if (stack.length > 1) {
    warnings.push(
      `unterminated string/template/comment at EOF (open context: ${stack[stack.length - 1].type}); ` +
        `kept as-is (safe over-count) - verify the strip heuristic.`,
    );
  }
  return masked;
}

// Production code LOC for one file's text: mask strings/templates/comments,
// then count remaining lines that still hold a non-space character.
export function prodLoc(text, warnings = []) {
  const lines = text.split('\n');
  const masked = maskNonCode(lines, warnings);
  let count = 0;
  for (const line of masked) if (/\S/.test(line)) count++;
  return count;
}

function relPosix(full) {
  return path.relative(REPO_ROOT, full).split(path.sep).join('/');
}

// Map repo-relative path -> production LOC for every scoped .ts/.tsx file.
// Symlinks in scope land in `symlinkErrors` (each fails the gate).
function measureTree(warnings = [], symlinkErrors = []) {
  const out = {};
  const { files, symlinks } = listScopedFiles(REPO_ROOT);
  for (const link of symlinks) symlinkErrors.push(symlinkProblem(link));
  for (const rel of files) out[rel] = prodLoc(fs.readFileSync(path.join(REPO_ROOT, rel), 'utf8'), warnings);
  return out;
}

function loadBaseline() {
  if (!fs.existsSync(BASELINE_PATH)) return {};
  try {
    return JSON.parse(fs.readFileSync(BASELINE_PATH, 'utf8'));
  } catch (e) {
    // A corrupted baseline (bad merge markers, truncated write) should fail with a
    // clean diagnostic, not an uncaught SyntaxError. `--update` rebuilds from
    // scratch and never calls this, so it can always self-heal a corrupt file.
    console.error(
      `check-ts-file-size: ${relPosix(BASELINE_PATH)} is not valid JSON (${e.code || e.message}). ` +
        `Fix it, or regenerate with: pnpm lint:ts-file-size --update`,
    );
    process.exit(1);
  }
}

function writeBaseline(measured) {
  const files = {};
  for (const k of Object.keys(measured).sort()) files[k] = measured[k];
  const payload = {
    _README:
      'Generated by scripts/check-ts-file-size.mjs --update. Production code LOC ' +
      '(blank/comment lines stripped) per .ts/.tsx file: non-test files under src ' +
      '(generated IPC types and the test harness excluded) and every file under ' +
      'e2e-playwright and e2e (generated e2e/agent/replays excluded), found by a ' +
      'filesystem walk (untracked files included, matching the Rust ratchet). warn_loc/fail_loc ' +
      'echo the script constants (the script asserts they match). The CI `check` ' +
      'job runs the script in check mode: a file at/above warn_loc is frozen (may ' +
      'shrink, not grow), no new file may be born at/above warn_loc, and no ' +
      'new/grown file may cross fail_loc. To move a baseline deliberately (a split ' +
      'lowered it, or a real feature grew a file), run --update and commit the diff.',
    warn_loc: WARN_LOC,
    fail_loc: FAIL_LOC,
    files,
  };
  fs.writeFileSync(BASELINE_PATH, JSON.stringify(payload, null, 2) + '\n');
}

function isAllowed(rel) {
  return ALLOW.some((a) => a.file === rel);
}

// Pure classification of one file against its baseline. level in 'ok'|'warn'|'error'.
export function classify(rel, loc, base) {
  const isNew = base === undefined;
  const grew = !isNew && loc > base;
  if (isAllowed(rel)) {
    // An allow-listed file is a documented irreducible exception: exempt from the
    // warn/new/hard-cap thresholds, but it still may NOT grow past its baseline -
    // an exception is not a licence to balloon without limit.
    return grew
      ? {
          level: 'error',
          message:
            `${rel}: ${loc} LOC > baseline ${base} - allow-listed files are exempt from the ` +
            `size budget but may not grow; split it, or run --update if the growth is intentional.`,
        }
      : { level: 'ok' };
  }
  if (loc > FAIL_LOC && (isNew || grew)) {
    return {
      level: 'error',
      message:
        `${rel}: ${loc} LOC exceeds the hard ${FAIL_LOC}-prod-LOC budget - split it into ` +
        `logical modules/components at an API boundary (#157-style split), or run --update ` +
        `if the growth is intentional.`,
    };
  }
  if (grew && loc >= WARN_LOC) {
    return {
      level: 'error',
      message:
        `${rel}: ${loc} LOC > baseline ${base} - a file at/above ${WARN_LOC} prod LOC may ` +
        `shrink but not grow. Split it (#157-style split), or run --update to rebaseline if ` +
        `the growth is intentional.`,
    };
  }
  if (isNew && loc >= WARN_LOC) {
    return {
      level: 'error',
      message:
        `${rel}: new file at ${loc} prod LOC (>= warn ${WARN_LOC}) - new files may not be ` +
        `introduced at/above the warn line. Split it, or run --update to baseline it ` +
        `deliberately (#157-style split).`,
    };
  }
  if (loc >= WARN_LOC) return { level: 'warn', message: `${rel}: ${loc} LOC (>= warn ${WARN_LOC})` };
  return { level: 'ok' };
}

function main() {
  if (!fs.existsSync(SRC_DIR)) {
    console.error(`check-ts-file-size: ${relPosix(SRC_DIR)} not found.`);
    return 1;
  }

  const warnings = [];
  const symlinkErrors = [];
  const measured = measureTree(warnings, symlinkErrors);

  // --update rebuilds the baseline from scratch (writing the script's own
  // thresholds), so it neither loads nor validates the old file - it always
  // self-heals a corrupt or drifted baseline.
  if (process.argv.includes('--update')) {
    writeBaseline(measured);
    const tracked = Object.values(measured).filter((v) => v >= WARN_LOC).length;
    console.log(
      `baseline updated: ${Object.keys(measured).length} files recorded ` +
        `(${tracked} at/above warn=${WARN_LOC}). wrote ${relPosix(BASELINE_PATH)}`,
    );
    for (const w of warnings) console.warn(`  ⚠ ${w}`);
    return 0;
  }

  const baseline = loadBaseline();
  // The script owns the thresholds; a drifted echo in the generated baseline is a
  // tampering smell, not a config knob - fail loudly instead of honoring it.
  for (const [key, want] of [['warn_loc', WARN_LOC], ['fail_loc', FAIL_LOC]]) {
    if (baseline[key] !== undefined && Number(baseline[key]) !== want) {
      console.error(
        `check-ts-file-size: baseline ${key}=${baseline[key]} disagrees with the script ` +
          `default ${want}. Thresholds are owned by the script; run --update to re-sync the ` +
          `baseline JSON (do not hand-edit it to change the gate).`,
      );
      return 1;
    }
  }

  const recorded = baseline.files ?? {};
  const errors = [...symlinkErrors];
  const softWarnings = [];

  for (const rel of Object.keys(measured).sort()) {
    const verdict = classify(rel, measured[rel], recorded[rel]);
    if (verdict.level === 'error') errors.push(verdict.message);
    else if (verdict.level === 'warn') softWarnings.push(verdict.message);
  }

  // Baselined files that no longer exist on disk (split/renamed/deleted) linger as
  // dead rows and mask whether a split actually shrank the tree. They don't affect
  // the gate (the loop above iterates `measured`), but flag them so a split PR
  // runs --update to prune them.
  const stale = Object.keys(recorded).filter((r) => measured[r] === undefined);
  if (stale.length > 0) {
    const shown = stale.slice(0, 5).join(', ');
    console.warn(
      `  ⚠ baseline: ${stale.length} recorded file(s) no longer exist ` +
        `(${shown}${stale.length > 5 ? ', …' : ''}); run --update to prune.`,
    );
  }

  for (const w of warnings) console.warn(`  ⚠ strip-heuristic: ${w}`);
  for (const w of softWarnings) console.log(`  ⚠ file-size: ${w}`);

  if (errors.length > 0) {
    console.error(`\n✗ TS file-size check failed - ${errors.length} issue(s):\n`);
    for (const e of errors) console.error(`  ${e}`);
    console.error(
      '\nProduction files must stay within the size budget; split at a component/module\n' +
        'boundary (#157-style split).\n' +
        'To rebaseline an intentional change: pnpm lint:ts-file-size --update\n',
    );
    return 1;
  }

  console.log(
    `✓ TS file-size: ${Object.keys(measured).length} files checked (src, e2e-playwright, e2e), ` +
      `none over the ${FAIL_LOC}-LOC budget or grown past baseline ` +
      `(${softWarnings.length} at/above the ${WARN_LOC} warn line).`,
  );
  return 0;
}

const isMain =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) process.exit(main());
