// File-size ratchet for the Rust workspace: the root `stalagmight` crate
// (src-tauri/src) plus every workspace member crate under src-tauri/crates/*/src
// (discovered at runtime; the crates/ dir may not exist yet).
//
// Keeps non-test `.rs` files from re-bloating after the module-split refactor
// (#157). The crate had files up to ~7k production LOC, which raises agent-context
// cost and hurts readability; this guard stops them growing back.
//
// Metric: PRODUCTION code LOC = source lines minus blank/comment lines, with
// `#[cfg(test)]`-gated items (both `mod tests { … }` blocks and braceless
// `#[cfg(test)] use …;` items) stripped.
//
// Whole TEST files are measured too, against their own, larger thresholds
// (#576 E8-D1 - they used to be exempt by name, 34 files / ~100k lines, the
// largest 26k): TEST_WARN_LOC / TEST_FAIL_LOC below, same freeze policy, code
// LOC with nothing stripped (the whole file is test code). A file is a test
// file when it lives under `src-tauri/tests/` (cargo integration tests), or
// when its name matches TEST_FILE_RE (tests.rs, *_tests.rs, *_fuzz.rs,
// test_helpers.rs) AND a sibling/parent module declares it under a test cfg
// (`#[cfg(test)] mod x;`, also `all(test, ..)` / `any(test, ..)`). A
// production file merely NAMED `foo_tests.rs` is therefore measured as
// production code - a name alone no longer moves a file to the looser caps.
//
// Scan roots: src-tauri/src and every src-tauri/crates/*/src, plus the
// first-party Rust outside them (#576 E8): src-tauri/build.rs and
// crates/*/build.rs, src-tauri/examples, src-tauri/tests, the agent harness's
// OS helper (e2e/agent/os/helper/src) and scripts/lib/slice-rs.
//
// Policy (baseline = src-tauri/file-size-baseline.json):
//   - A file at/above `WARN_LOC` (1500) prod LOC is "large" and FROZEN by the
//     ratchet: it may shrink, never grow past its recorded baseline. Split it at
//     an API boundary, or run `--update` to rebaseline an intentional change.
//   - A NEW (non-baselined) file may not be introduced at/above `WARN_LOC` -
//     no new god-files born just under the cap.
//   - A NEW or GROWN file may never exceed the hard `FAIL_LOC` (2000) budget.
//   - Files below `WARN_LOC` grow freely (so ordinary edits to small files don't
//     need a rebaseline); grandfathered files already over the cap only need to
//     stop growing.
// Anything at/above `WARN_LOC` that isn't a violation prints a non-fatal warning.
//
// Keeping the baseline complete (#576 NF-W9E1-2): a new file below its warn line
// needs no row, so rows went missing (in Nov 2026 only 37 of 191 test files had
// one) and shrunk files kept their old, larger rows - `--update` would have fixed
// both but also RAISES every row whose file grew below warn, a loosening nobody
// wants in an unrelated diff, so rows were hand-edited instead. `--tighten` is the safe refresh: it lowers a
// row to its file's size, adds a row for an unrecorded file and prunes a row
// whose file is gone, and never raises a row. Check mode notes unrecorded files.
// [#576 RV10a-2] It never adds a row for a NEW file at/above its warn line: that
// row would grandfather exactly the file check mode refuses, so --tighten fails
// naming it, with the split instruction, and writes nothing (`--update` stays the
// deliberate way to baseline one).
//
// Runs in the `check` job (ci.yml) - which runs on `src-tauri/**` changes and is
// NOT path-ignored (unlike `smoke`), so it actually sees Rust edits. The strip
// heuristic is pinned by scripts/check-rust-file-size.test.mjs. Mirrors the
// existing tripwire `scripts/check-e2e-parallel-safe.mjs`.
//
// Usage:  node scripts/check-rust-file-size.mjs            (pnpm lint:rust-file-size)
//         node scripts/check-rust-file-size.mjs --tighten  (lower / add / prune rows; never raise)
//         node scripts/check-rust-file-size.mjs --update   (regenerate the baseline)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const TAURI_DIR = path.join(REPO_ROOT, 'src-tauri');
const BASELINE_PATH = path.join(REPO_ROOT, 'src-tauri', 'file-size-baseline.json');

// Scan roots: the root crate's src/ plus every workspace member's src/ under
// crates/*. Discovered at runtime so an extraction commit cannot silently move
// a file out of the ratchet's sight; tolerates crates/ not existing (the
// workspace bootstrap lands before any member is extracted).
export function discoverSrcRoots(base = TAURI_DIR) {
  const roots = [];
  const primary = path.join(base, 'src');
  if (fs.existsSync(primary)) roots.push(primary);
  const cratesDir = path.join(base, 'crates');
  let entries;
  try {
    entries = fs.readdirSync(cratesDir, { withFileTypes: true });
  } catch {
    return roots; // no crates/ yet
  }
  for (const e of entries.sort((a, b) => a.name.localeCompare(b.name))) {
    if (!e.isDirectory()) continue;
    const src = path.join(cratesDir, e.name, 'src');
    if (fs.existsSync(src)) roots.push(src);
  }
  return roots;
}

// First-party Rust outside the crate src/ trees (#576 E8): {files, dirs,
// testDirs}, repo-relative, only those that exist. `testDirs` hold cargo
// integration tests, which are test code by location.
export function discoverExtraRoots(repoRoot = REPO_ROOT) {
  const files = [];
  const exists = (rel) => fs.existsSync(path.join(repoRoot, rel));
  if (exists('src-tauri/build.rs')) files.push('src-tauri/build.rs');
  let crates = [];
  try {
    crates = fs.readdirSync(path.join(repoRoot, 'src-tauri', 'crates'), { withFileTypes: true });
  } catch {
    // no crates/ yet
  }
  for (const e of crates.sort((a, b) => a.name.localeCompare(b.name))) {
    const rel = `src-tauri/crates/${e.name}/build.rs`;
    if (e.isDirectory() && exists(rel)) files.push(rel);
  }
  const dirs = ['src-tauri/examples', 'e2e/agent/os/helper/src', 'scripts/lib/slice-rs'].filter(exists);
  const testDirs = ['src-tauri/tests'].filter(exists);
  return { files, dirs, testDirs };
}

// Thresholds are owned by the script (the single source of truth). The baseline
// JSON echoes them for readability, but `main()` asserts the echo matches these
// and `--update` always writes these - so editing the generated artifact can't
// quietly loosen the *global thresholds*. (A per-file baseline number can still be
// bumped to grant a specific file room, but that shows up in the baseline diff
// right next to the grown file, so it's review-visible, not silent.)
export const WARN_LOC = 1500;
export const FAIL_LOC = 2000;

// Test-file thresholds (#576 E8-D1), set after measuring the 34 test files in
// Nov 2026 (code LOC): 21464, 12266, 6864, 4817, 4789, 4667, then 2893 and
// below. At 3000 the six monoliths are frozen (may shrink, never grow - new
// tests go into new submodules, E1) and a new test file cannot be born a
// monolith; 5000 is the hard cap for any new or grown test file.
export const TEST_WARN_LOC = 3000;
export const TEST_FAIL_LOC = 5000;

export const PROD_LIMITS = { warn: WARN_LOC, fail: FAIL_LOC, kind: 'production' };
export const TEST_LIMITS = { warn: TEST_WARN_LOC, fail: TEST_FAIL_LOC, kind: 'test' };

// Names a whole-file test module may have. A name alone is not enough - see
// isTestModule: the parent must also declare the module under a test cfg.
export const TEST_FILE_RE = /(^|\/)(tests\.rs|.*_tests\.rs|.*_fuzz\.rs|test_helpers\.rs)$/;

// `#[cfg(test)]`, `#[cfg(all(test, …))]`, `#[cfg(any(test, …))]` - the test-gated
// forms whose items we strip. NOT `#[cfg_attr(test, …)]` (that conditionally
// applies an attribute to a *production* item) and NOT `#[cfg(not(test))]` (the
// production-only form) - both correctly fall through and stay counted.
const CFG_TEST_RE = /#\[cfg\((?:test\b|all\(test|any\(test)/;

// Does `parentText` declare `mod <stem>;` under a test cfg? Strings/comments
// are masked first (so a `]` inside an #[expect(reason = "..")] string cannot
// break the attribute walk), then every attribute stacked on the declaration
// is checked.
export function declaredUnderTestCfg(parentText, stem) {
  const code = maskNonCode(parentText.split('\n')).join('\n');
  const re = new RegExp(`\\b(?:pub(?:\\s*\\([^)]*\\))?\\s+)?mod\\s+${stem}\\s*;`, 'g');
  let m;
  while ((m = re.exec(code)) !== null) {
    let i = m.index - 1;
    while (true) {
      while (i >= 0 && /\s/.test(code[i])) i--;
      if (i < 0 || code[i] !== ']') break;
      let depth = 0;
      let j = i;
      for (; j >= 0; j--) {
        if (code[j] === ']') depth++;
        else if (code[j] === '[' && --depth === 0) break;
      }
      if (j < 1 || code[j - 1] !== '#') break;
      if (CFG_TEST_RE.test(code.slice(j - 1, i + 1))) return true;
      i = j - 2;
    }
  }
  return false;
}

// A whole-file test module: TEST_FILE_RE name AND declared under a test cfg by
// its parent (mod.rs / lib.rs / main.rs / a sibling in the same dir, or the
// `<dir>.rs` next to the dir). `read` is injectable for the fixture test.
export function isTestModule(full, read = (f) => fs.readFileSync(f, 'utf8'), list = (d) => fs.readdirSync(d)) {
  if (!TEST_FILE_RE.test(full.split(path.sep).join('/'))) return false;
  const stem = path.basename(full, '.rs');
  const dir = path.dirname(full);
  let siblings = [];
  try {
    siblings = list(dir).filter((n) => n.endsWith('.rs')).map((n) => path.join(dir, n));
  } catch {
    // unreadable dir: no sibling parents
  }
  const candidates = [...new Set([`${dir}.rs`, ...siblings])].filter((p) => p !== full);
  for (const p of candidates) {
    let text;
    try {
      text = read(p);
    } catch {
      continue;
    }
    if (declaredUnderTestCfg(text, stem)) return true;
  }
  return false;
}

// Files allowed to exceed the budget without ratcheting, each with a written
// reason. The baseline already grandfathers today's large files, so this is only
// for a future file that is genuinely irreducible. Add a row with a justification.
const ALLOW = [];

// Mask string-literal and comment spans so downstream brace/`;` counting sees only
// real code, while keeping the LOC count faithful. Comments → spaces (a
// comment-only line becomes blank, so it isn't counted). String/char content →
// a non-whitespace placeholder `M` (so a line of multi-line-string body still
// counts as code, like tokei, but its braces/`;` don't perturb the strip scan).
// Handles // line comments, /* */ (nesting), "…" (with \ escapes, multi-line),
// raw strings r#*"…"#*, byte variants (b"…", br#"…"#), and char literals ('{',
// '\n') vs lifetimes ('a). Factored out so the same state machine feeds both the
// LOC count and the #[cfg(test)] strip - the previous per-line brace counter saw
// string braces and could run a strip to EOF (a silent under-count).
const MASK = 'M'; // non-whitespace, non-brace, non-`;` placeholder for string/char bytes
function maskStr(s) {
  return s.replace(/\S/g, MASK); // keep whitespace (so an all-space string line stays blank)
}
export function maskNonCode(lines) {
  const masked = [];
  let blockDepth = 0; // /* */ nesting depth, persists across lines
  let str = null; // null | {raw:false} | {raw:true, hashes:N}, persists across lines
  for (const line of lines) {
    const n = line.length;
    let out = '';
    let i = 0;
    while (i < n) {
      const c = line[i];
      const c2 = line[i + 1];
      if (blockDepth > 0) {
        if (c === '/' && c2 === '*') { blockDepth++; out += '  '; i += 2; continue; }
        if (c === '*' && c2 === '/') { blockDepth--; out += '  '; i += 2; continue; }
        out += ' '; i++; continue;
      }
      if (str) {
        if (str.raw) {
          const close = '"' + '#'.repeat(str.hashes);
          if (line.startsWith(close, i)) { out += maskStr(close); i += close.length; str = null; continue; }
          out += maskStr(c); i++; continue;
        }
        if (c === '\\') { out += maskStr(line.slice(i, i + 2)); i += 2; continue; } // escape (incl. line continuation)
        if (c === '"') { out += MASK; i++; str = null; continue; }
        out += maskStr(c); i++; continue;
      }
      if (c === '/' && c2 === '/') { out += ' '.repeat(n - i); break; } // line comment → rest of line
      if (c === '/' && c2 === '*') { blockDepth = 1; out += '  '; i += 2; continue; }
      const rest = line.slice(i);
      const rawStart = /^b?r(#*)"/.exec(rest);
      if (rawStart) {
        str = { raw: true, hashes: rawStart[1].length };
        out += maskStr(rawStart[0]); i += rawStart[0].length; continue;
      }
      const strStart = /^b?"/.exec(rest);
      if (strStart) {
        str = { raw: false };
        out += maskStr(strStart[0]); i += strStart[0].length; continue;
      }
      if (c === "'") {
        // '{' / '\n' / '\u{7d}' / '\x7d' - a char literal (masked). A bare 'a is a
        // lifetime/label: keep the ' (harmless; carries no brace).
        const charLit = /^'(?:\\u\{[0-9a-fA-F]*\}|\\x[0-9a-fA-F]{1,2}|\\.|[^'])'/.exec(rest);
        if (charLit) { out += maskStr(charLit[0]); i += charLit[0].length; continue; }
        out += c; i++; continue;
      }
      out += c; i++;
    }
    masked.push(out);
  }
  return masked;
}

// Given code-only (masked) lines, return a boolean[] marking lines that belong to
// a `#[cfg(test)]` item - block (`mod tests { … }`) or braceless (`use …;`). On a
// malformed/unclosed scan it keeps the lines (safe over-count) and records a
// warning rather than silently dropping everything to EOF.
export function markCfgTestItems(masked, warnings = []) {
  const drop = new Array(masked.length).fill(false);
  let i = 0;
  while (i < masked.length) {
    if (!CFG_TEST_RE.test(masked[i])) { i++; continue; }
    let depth = 0;
    let sawBrace = false;
    let endLine = -1;
    let endCol = -1;
    for (let j = i; j < masked.length && endLine < 0; j++) {
      const row = masked[j];
      for (let c = 0; c < row.length; c++) {
        const ch = row[c];
        if (ch === '{') { depth++; sawBrace = true; }
        else if (ch === '}') { depth--; }
        else if (ch === ';' && depth === 0 && !sawBrace) { endLine = j; endCol = c; break; } // braceless item ends
        if (sawBrace && depth === 0) { endLine = j; endCol = c; break; } // block item closed
      }
    }
    if (endLine >= 0) {
      // If real code follows the terminator on the end line (non-idiomatic line
      // mixing - rustfmt always splits these, but don't rely on that for the
      // dangerous direction - e.g. `#[cfg(test)] use x; fn prod() {}`), keep the
      // end line so the trailing production isn't dropped (over-count is safe).
      const trailingCode = /\S/.test(masked[endLine].slice(endCol + 1));
      const lastDrop = trailingCode ? endLine - 1 : endLine;
      for (let k = i; k <= lastDrop; k++) drop[k] = true;
      i = endLine + 1;
    } else {
      if (sawBrace) {
        warnings.push(
          `unbalanced #[cfg(test)] block starting near line ${i + 1}; kept as production ` +
            `LOC (safe over-count) - verify the strip heuristic.`,
        );
      }
      i++; // keep the attribute line, continue scanning
    }
  }
  return drop;
}

// Production code LOC for one file's text: mask strings/comments, drop #[cfg(test)]
// items, then count remaining lines that still hold a non-space character.
export function prodLoc(text, warnings = []) {
  const lines = text.split('\n');
  const masked = maskNonCode(lines);
  const drop = markCfgTestItems(masked, warnings);
  let count = 0;
  for (let k = 0; k < lines.length; k++) {
    if (!drop[k] && /\S/.test(masked[k])) count++;
  }
  return count;
}

function listRustFiles(dir) {
  const out = [];
  const entries = fs
    .readdirSync(dir, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name));
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...listRustFiles(full));
    else if (entry.isFile() && entry.name.endsWith('.rs')) out.push(full);
  }
  return out;
}

// Code LOC for a whole test file: comments/blank lines masked, nothing else
// stripped (the file is test code throughout).
export function testLoc(text) {
  return maskNonCode(text.split('\n')).filter((l) => /\S/.test(l)).length;
}

// [#576 NF-W9E1-2] The rows `--tighten` writes for one section (`files` or
// `test_files`): every measured file gets a row, its recorded size when that is
// not larger than the measured one, else the measured size - so a row only ever
// goes down. A file with no row is recorded at its size; a row whose file is gone
// is dropped (it is not in `measured`). Pure, so the fixture test can pin it.
//
// [#576 RV10a-2] Except a file with no row at/above the section's warn line
// (`limits`): check mode refuses it as a new file born too large, and a row would
// turn that refusal into a pass. It gets no row; `--tighten` fails naming it.
export function tightenRows(recorded, measured, limits = PROD_LIMITS) {
  const rows = {};
  for (const rel of Object.keys(measured).sort()) {
    const was = recorded[rel];
    if (was !== undefined) rows[rel] = Math.min(Number(was), measured[rel]);
    else if (!bornTooLarge(rel, measured[rel], limits)) rows[rel] = measured[rel];
  }
  return rows;
}

// [#576 RV10a-2] A file with no row that check mode refuses for its size alone
// (`classify`'s new-file-at/above-warn and hard-cap errors). Allow-listed files are
// exempt from the warn line, so recording one is still a tightening.
function bornTooLarge(rel, loc, limits) {
  return !isAllowed(rel) && loc >= limits.warn;
}

// [#576 NF-W9E1-2] Measured files that have no baseline row, sorted.
export function unrecordedFiles(recorded, measured) {
  return Object.keys(measured)
    .filter((rel) => recorded[rel] === undefined)
    .sort();
}

function relPosix(full) {
  return path.relative(REPO_ROOT, full).split(path.sep).join('/');
}

// {prod, tests}: repo-relative path -> LOC, for every first-party .rs file in
// scope (see the header). Test files go to `tests`, measured by testLoc.
function measureTree(warnings = []) {
  const prod = {};
  const tests = {};
  const extra = discoverExtraRoots();
  const prodFiles = [
    ...discoverSrcRoots().flatMap((root) => listRustFiles(root)),
    ...extra.dirs.flatMap((d) => listRustFiles(path.join(REPO_ROOT, d))),
    ...extra.files.map((f) => path.join(REPO_ROOT, f)),
  ];
  for (const full of prodFiles) {
    const text = fs.readFileSync(full, 'utf8');
    if (isTestModule(full)) tests[relPosix(full)] = testLoc(text);
    else prod[relPosix(full)] = prodLoc(text, warnings);
  }
  for (const d of extra.testDirs) {
    for (const full of listRustFiles(path.join(REPO_ROOT, d))) tests[relPosix(full)] = testLoc(fs.readFileSync(full, 'utf8'));
  }
  return { prod, tests };
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
      `check-rust-file-size: ${relPosix(BASELINE_PATH)} is not valid JSON (${e.code || e.message}). ` +
        `Fix it, or regenerate with: pnpm lint:rust-file-size --update`,
    );
    process.exit(1);
  }
}

function writeBaseline({ prod, tests }) {
  const sorted = (m) => Object.fromEntries(Object.keys(m).sort().map((k) => [k, m[k]]));
  const payload = {
    _README:
      'Generated by scripts/check-rust-file-size.mjs --update. `files`: production code LOC ' +
      '(blank/comment lines and #[cfg(test)] items stripped) per non-test .rs file under ' +
      'src-tauri/src, src-tauri/crates/*/src and the other first-party Rust roots the script ' +
      'lists. `test_files`: code LOC of each whole test file (src-tauri/tests, or a ' +
      'cfg(test)-declared tests.rs / *_tests.rs / *_fuzz.rs / test_helpers.rs), held to the ' +
      'separate test_warn_loc/test_fail_loc. The *_loc keys echo the script constants (the ' +
      'script asserts they match). The CI `check` job runs the script in check mode: a file ' +
      'at/above its warn line is frozen (may shrink, not grow), no new file may be born ' +
      'at/above it, and no new/grown file may cross its fail line. To record new files ' +
      'and lower shrunk rows without raising any, run --tighten; to move a baseline ' +
      'deliberately (a real feature grew a frozen file), run --update. Commit the diff.',
    warn_loc: WARN_LOC,
    fail_loc: FAIL_LOC,
    test_warn_loc: TEST_WARN_LOC,
    test_fail_loc: TEST_FAIL_LOC,
    files: sorted(prod),
    test_files: sorted(tests),
  };
  fs.writeFileSync(BASELINE_PATH, JSON.stringify(payload, null, 2) + '\n');
}

function isAllowed(rel) {
  return ALLOW.some((a) => a.file === rel);
}

// Pure classification of one file against its baseline. level ∈ 'ok'|'warn'|'error'.
// `limits` selects the production (default) or test-file thresholds.
export function classify(rel, loc, base, limits = PROD_LIMITS) {
  const { warn, fail, kind } = limits;
  const split = kind === 'test'
    ? 'Move new tests into a new submodule (split the monolith, #576 E1/E8)'
    : 'Split it into logical modules at an API boundary (#157)';
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
  if (loc > fail && (isNew || grew)) {
    return {
      level: 'error',
      message:
        `${rel}: ${loc} LOC exceeds the hard ${fail}-LOC budget for ${kind} files - ${split}, ` +
        `or run --update if the growth is intentional.`,
    };
  }
  if (grew && loc >= warn) {
    return {
      level: 'error',
      message:
        `${rel}: ${loc} LOC > baseline ${base} - a ${kind} file at/above ${warn} LOC may ` +
        `shrink but not grow. ${split}, or run --update to rebaseline if the growth ` +
        `is intentional.`,
    };
  }
  if (isNew && loc >= warn) {
    return {
      level: 'error',
      message:
        `${rel}: new ${kind} file at ${loc} LOC (>= warn ${warn}) - new files may not be ` +
        `introduced at/above the warn line. ${split}, or run --update to baseline it ` +
        `deliberately.`,
    };
  }
  if (loc >= warn) return { level: 'warn', message: `${rel}: ${loc} LOC (>= ${kind} warn ${warn})` };
  return { level: 'ok' };
}

function main() {
  const primary = path.join(TAURI_DIR, 'src');
  if (!fs.existsSync(primary)) {
    console.error(`check-rust-file-size: ${relPosix(primary)} not found.`);
    return 1;
  }

  const warnings = [];
  const measured = measureTree(warnings);
  const nProd = Object.keys(measured.prod).length;
  const nTest = Object.keys(measured.tests).length;

  // --update rebuilds the baseline from scratch (writing the script's own
  // thresholds), so it neither loads nor validates the old file - it always
  // self-heals a corrupt or drifted baseline.
  if (process.argv.includes('--update')) {
    writeBaseline(measured);
    const frozen = Object.values(measured.prod).filter((v) => v >= WARN_LOC).length;
    const frozenTests = Object.values(measured.tests).filter((v) => v >= TEST_WARN_LOC).length;
    console.log(
      `baseline updated: ${nProd} production files (${frozen} at/above warn=${WARN_LOC}) and ` +
        `${nTest} test files (${frozenTests} at/above test warn=${TEST_WARN_LOC}). wrote ${relPosix(BASELINE_PATH)}`,
    );
    for (const w of warnings) console.warn(`  ⚠ ${w}`);
    return 0;
  }

  const baseline = loadBaseline();
  // The script owns the thresholds; a drifted echo in the generated baseline is a
  // tampering smell, not a config knob - fail loudly instead of honoring it.
  for (const [key, want] of [['warn_loc', WARN_LOC], ['fail_loc', FAIL_LOC], ['test_warn_loc', TEST_WARN_LOC], ['test_fail_loc', TEST_FAIL_LOC]]) {
    if (baseline[key] !== undefined && Number(baseline[key]) !== want) {
      console.error(
        `check-rust-file-size: baseline ${key}=${baseline[key]} disagrees with the script ` +
          `default ${want}. Thresholds are owned by the script; run --update to re-sync the ` +
          `baseline JSON (do not hand-edit it to change the gate).`,
      );
      return 1;
    }
  }

  // --tighten rewrites the baseline from the recorded rows, lowered / added /
  // pruned by `tightenRows` - never raised - after the threshold echo above has
  // been validated, so it cannot launder a tampered baseline.
  if (process.argv.includes('--tighten')) {
    const recordedProd = baseline.files ?? {};
    const recordedTests = baseline.test_files ?? {};
    const prod = tightenRows(recordedProd, measured.prod, PROD_LIMITS);
    const tests = tightenRows(recordedTests, measured.tests, TEST_LIMITS);
    // [#576 RV10a-2] A measured file without a row (it is not in `rows`) was refused.
    const refused = [
      [recordedProd, measured.prod, prod, PROD_LIMITS],
      [recordedTests, measured.tests, tests, TEST_LIMITS],
    ].flatMap(([recorded, section, rows, limits]) =>
      unrecordedFiles(recorded, section)
        .filter((rel) => rows[rel] === undefined)
        .map((rel) => classify(rel, section[rel], undefined, limits).message),
    );
    if (refused.length > 0) {
      console.error(
        `\n✗ --tighten refused - ${refused.length} new file(s) at/above a warn line; ` +
          `it never records one (that would grandfather the file the check refuses):\n`,
      );
      for (const m of refused) console.error(`  ${m}`);
      console.error(`\nNothing was written to ${relPosix(BASELINE_PATH)}.\n`);
      return 1;
    }
    const count = (recorded, rows) => ({
      lowered: Object.keys(rows).filter((r) => recorded[r] !== undefined && rows[r] < Number(recorded[r])).length,
      added: unrecordedFiles(recorded, rows).length,
      pruned: Object.keys(recorded).filter((r) => rows[r] === undefined).length,
    });
    for (const [label, c] of [['production', count(recordedProd, prod)], ['test', count(recordedTests, tests)]]) {
      console.log(`baseline tightened (${label}): ${c.lowered} row(s) lowered, ${c.added} added, ${c.pruned} pruned, none raised`);
    }
    writeBaseline({ prod, tests });
    console.log(`wrote ${relPosix(BASELINE_PATH)}`);
    return 0;
  }

  const errors = [];
  const softWarnings = [];
  const stale = [];
  const unrecorded = [];
  for (const [section, limits, recorded] of [
    [measured.prod, PROD_LIMITS, baseline.files ?? {}],
    [measured.tests, TEST_LIMITS, baseline.test_files ?? {}],
  ]) {
    for (const rel of Object.keys(section).sort()) {
      const verdict = classify(rel, section[rel], recorded[rel], limits);
      if (verdict.level === 'error') errors.push(verdict.message);
      else if (verdict.level === 'warn') softWarnings.push(verdict.message);
    }
    // Baselined files that no longer exist on disk (split/renamed/deleted) linger as
    // dead rows and mask whether a split actually shrank the tree. They don't affect
    // the gate (the loop above iterates `measured`), but flag them so a split PR
    // runs --tighten to prune them.
    stale.push(...Object.keys(recorded).filter((r) => section[r] === undefined));
    // [#576 NF-W9E1-2] Files with no row pass below their warn line, so nothing
    // made anyone record them; say so, the same way. [#576 RV10a-2] Only those:
    // one at/above the line is an error above, and --tighten refuses to record it.
    unrecorded.push(
      ...unrecordedFiles(recorded, section).filter((rel) => !bornTooLarge(rel, section[rel], limits)),
    );
  }
  if (stale.length > 0) {
    const shown = stale.slice(0, 5).join(', ');
    console.warn(
      `  ⚠ baseline: ${stale.length} recorded file(s) no longer exist ` +
        `(${shown}${stale.length > 5 ? ', …' : ''}); run --tighten to prune.`,
    );
  }
  if (unrecorded.length > 0) {
    const shown = unrecorded.slice(0, 5).join(', ');
    console.warn(
      `  ⚠ baseline: ${unrecorded.length} measured file(s) have no row ` +
        `(${shown}${unrecorded.length > 5 ? ', …' : ''}); run --tighten to record them ` +
        `(it never raises a row).`,
    );
  }

  for (const w of warnings) console.warn(`  ⚠ strip-heuristic: ${w}`);
  for (const w of softWarnings) console.log(`  ⚠ file-size: ${w}`);

  if (errors.length > 0) {
    console.error(`\n✗ Rust file-size check failed - ${errors.length} issue(s):\n`);
    for (const e of errors) console.error(`  ${e}`);
    console.error(
      '\nFiles must stay within their size budget; see #157 for the split plan.\n' +
        'To rebaseline an intentional change: pnpm lint:rust-file-size --update\n',
    );
    return 1;
  }

  console.log(
    `✓ Rust file-size: ${nProd} production files (budget ${FAIL_LOC}) and ${nTest} test files ` +
      `(budget ${TEST_FAIL_LOC}) checked, none over budget or grown past baseline ` +
      `(${softWarnings.length} at/above a warn line, frozen).`,
  );
  return 0;
}

const isMain =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) process.exit(main());
