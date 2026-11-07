// Parallel-safety guard for the Playwright tier (issue #126).
//
// The `e2e-playwright/` tier runs with `fullyParallel: true` at multiple workers.
// That is safe ONLY because every spec keeps its backend state per-page on
// `window.*` (mocked IPC) - no spec binds a port, writes a fixed disk path, or
// shares a server / global. This scanner fails the build if a spec introduces a
// pattern that would break that invariant. See docs/testing-recipes.md.
//
// Runs in the `smoke` job (e2e-smoke.yml) - the only CI job that runs the
// Playwright tier on `e2e-playwright/**` changes. `check` (the home of the
// repo's Rust/vitest grep-gates) path-ignores `e2e-playwright/**`, so a gate
// there would be a no-op for an e2e-playwright-only PR - exactly the change this
// must catch. Its rules are pinned by scripts/check-e2e-parallel-safe.test.mjs
// (#576 E7), which runs first in the same job.
//
// The same walk over every spec also checks one tier invariant that is not about
// parallelism (#576 RV8b-E3-3): each spec takes `test` from fixtures/coverage.ts,
// whose auto fixture is the unmocked-IPC teardown guard (rule `unguarded-test`).
//
// The premise is checked too: the root `playwright.config.ts` must still say
// `fullyParallel: true` - a config that quietly turned it off would make every
// rule below moot while the gate kept passing.
//
// This is a line-based heuristic tripwire for the *common, accidental* regression,
// not a parser/sandbox. Comments are stripped by a small string-aware scanner
// (a quoted `/*` or `//` neither opens nor cuts a comment; block comments carry
// across lines), so a pattern named in prose does not fire while one inside a
// string does. Known accepted gaps: it doesn't inspect `page.screenshot({path})`
// (framework I/O, used with unique filenames) or `process.env`/module-global
// mutation. Regex literals are recognised, so a `/*` or a quote inside one
// neither opens a comment nor hides code (scripts/lib/js-comments.mjs).
//
// Usage:  node scripts/check-e2e-parallel-safe.mjs   (or: pnpm lint:e2e-parallel)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { walkFiles } from './lib/src-files.mjs';
import { stripComments } from './lib/js-comments.mjs';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const SCAN_DIR = 'e2e-playwright';
export const CONFIG_FILE = 'playwright.config.ts';
const SHARED_VITE_PORT = '1420'; // the one shared webServer every page talks to

// Files allowed to break a specific rule, each with the reason it is safe. The
// coverage fixture writes a per-(test,worker)-unique file under docs/coverage,
// gated behind E2E_COVERAGE=1 - parallel-safe by construction. To add a genuine
// exception, append a row here with a written justification.
export const ALLOW = [
  {
    file: 'e2e-playwright/fixtures/coverage.ts',
    rule: 'disk-write',
    reason: 'per-(test,worker)-unique path under docs/coverage, gated on E2E_COVERAGE=1',
  },
];

// The fs calls that write, create, delete or link on disk, and the short async
// names (fs/promises) that are also common method names elsewhere.
const DISK_WRITE_NAMES = [
  'writeFileSync', 'writeFile', 'appendFileSync', 'appendFile', 'createWriteStream', 'copyFileSync', 'copyFile', 'cpSync',
  'renameSync', 'mkdirSync', 'mkdir', 'unlinkSync', 'unlink', 'rmSync', 'rmdirSync', 'rmdir', 'symlinkSync', 'symlink',
  'truncateSync', 'openSync', 'writeSync', 'writevSync', 'linkSync', 'chmodSync', 'ftruncateSync',
];
const FS_SHORT_NAMES = ['open', 'link', 'chmod', 'rm', 'cp', 'rename', 'truncate'];
const LONG = DISK_WRITE_NAMES.join('|');
const FS_RECEIVER = '\\b(?:fs|fsp|fsPromises|promises)';

// `{ ... }` import / destructure sources that bind fs functions to plain names:
// `import {...} from '(node:)fs(/promises)'`, and a destructure `{...} =` of
// `require(...)` / `await import(...)` of the same modules or of `fs | fsp |
// fsPromises | promises | fs.promises`. Matched on the whole comment-stripped
// file, so a multi-line brace counts. An import brace is flat (group 1 is its
// contents); a destructure may nest - `const { promises: { open } } = fs`
// (#576 NF-W9FE-4) - so FS_DESTRUCTURE_RE finds only its closing brace and the
// `=`, and the pattern is walked back to its opening brace.
const FS_IMPORT_RE = /\bimport\s+(?:type\s+)?(?:[\w$]+\s*,\s*)?\{([^}]*)\}\s*from\s*(['"`])(?:node:)?fs(?:\/promises)?\2/g;
// The fs module (or fs.promises) as a value: what a destructure or alias reads.
// `quote` is the number of the capture group that holds the module name's quote.
const fsSource = (quote) =>
  '(?:(?:await\\s+)?(?:require|import)\\s*\\(\\s*([\'"`])(?:node:)?fs(?:\\/promises)?\\' + quote +
  '\\s*\\)|(?:fs\\.promises|fsPromises|fsp|fs|promises)(?![\\w$.]))';
// The pattern may carry a type annotation (`}: typeof fs.promises =`, #576 RV10b-G6).
const FS_DESTRUCTURE_RE = new RegExp(String.raw`\}\s*(?::[^=;{}]+)?=(?![=>])\s*` + fsSource(1), 'g');
// A binding key in a destructure pattern: the name right after `{` or `,` at any
// depth (`writeFile` in `{ writeFile: w }`; not `w`, which follows a `:`), also
// quoted or computed (`{ 'open': o }`, `{ ['open']: o }`, #576 RV10b-G6).
const PATTERN_KEY_RE = /[{,]\s*(?:\[\s*(['"`])([A-Za-z_$][\w$]*)\1\s*\]|(['"`])([A-Za-z_$][\w$]*)\3|([A-Za-z_$][\w$]*))/g;
const keyName = (m) => m[2] ?? m[4] ?? m[5];
const FS_IMPORTED = new Set([...DISK_WRITE_NAMES, ...FS_SHORT_NAMES]);
// [#576 RV10b-G6] A new name for the fs module or fs.promises - `import * as X` /
// `import X from` '(node:)fs(/promises)', `const X = <fs source>`, or the
// `promises` key of an fs destructure renamed (`const { promises: p } = fs`) -
// is an fs receiver like `fs` itself, so its open / link / chmod count.
const FS_NAMESPACE_IMPORT_RE = /\bimport\s+(?:\*\s*as\s+)?([A-Za-z_$][\w$]*)\s+from\s*(['"`])(?:node:)?fs(?:\/promises)?\2/g;
const FS_ALIAS_RE = new RegExp(String.raw`\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=;{}]+)?=(?![=>])\s*` + fsSource(2), 'g');
const PROMISES_ALIAS_RE = /[{,]\s*promises\s*:\s*([A-Za-z_$][\w$]*)/g;
const RECEIVER_ONLY_NAMES = ['open', 'link', 'chmod'];

// The index of the `{` matching the `}` at `close`, scanning back; -1 if none.
function openingBrace(text, close) {
  let depth = 0;
  for (let i = close; i >= 0; i--) {
    if (text[i] === '}') depth++;
    else if (text[i] === '{' && --depth === 0) return i;
  }
  return -1;
}

// 0-based lines of `code` (stripComments output) that import or destructure a
// disk-write name from fs, alias or not, nested or not: the line of the name itself.
function fsImportLines(code) {
  const text = code.join('\n');
  const lines = new Set();
  const lineOf = (at) => text.slice(0, at).split('\n').length - 1;
  let m;
  FS_IMPORT_RE.lastIndex = 0;
  while ((m = FS_IMPORT_RE.exec(text)) !== null) {
    let at = m.index + m[0].indexOf('{') + 1;
    for (const entry of m[1].split(',')) {
      const name = /^\s*(?:type\s+)?([A-Za-z_$][\w$]*)/.exec(entry);
      if (name && FS_IMPORTED.has(name[1])) lines.add(lineOf(at + name[0].length));
      at += entry.length + 1;
    }
  }
  const aliases = new Set();
  // A destructure `{...} = <source>`: its fs names are disk-write lines, and a renamed
  // `promises` key is one more alias.
  const destructures = (re) => {
    re.lastIndex = 0;
    while ((m = re.exec(text)) !== null) {
      const open = openingBrace(text, m.index);
      if (open < 0) continue;
      const pattern = text.slice(open, m.index + 1);
      for (const key of pattern.matchAll(PATTERN_KEY_RE)) {
        if (FS_IMPORTED.has(keyName(key))) lines.add(lineOf(open + key.index + key[0].length));
      }
      for (const alias of pattern.matchAll(PROMISES_ALIAS_RE)) aliases.add(alias[1]);
    }
  };
  destructures(FS_DESTRUCTURE_RE);
  for (const re of [FS_NAMESPACE_IMPORT_RE, FS_ALIAS_RE]) {
    re.lastIndex = 0;
    while ((m = re.exec(text)) !== null) aliases.add(m[1]);
  }
  // [#576 RV12-F4] An alias is an fs source like `fs` itself: a destructure from it
  // (`const p = fs.promises; const { open } = p`) binds fs names, and `const q = p`
  // is one more alias. Walked to a fixpoint, each alias once.
  const walked = new Set();
  for (let next = [...aliases]; next.length > 0; next = [...aliases].filter((a) => !walked.has(a))) {
    for (const alias of next) {
      walked.add(alias);
      const id = alias.replace(/\$/g, '\\$');
      const source = `${id}(?![\\w$.])`;
      destructures(new RegExp(String.raw`\}\s*(?::[^=;{}]+)?=(?![=>])\s*` + source, 'g'));
      const rebind = new RegExp(String.raw`\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=;{}]+)?=(?![=>])\s*` + source, 'g');
      while ((m = rebind.exec(text)) !== null) aliases.add(m[1]);
    }
  }
  for (const alias of aliases) {
    const id = alias.replace(/\$/g, '\\$');
    const member = new RegExp(
      `(?<![\\w$.])${id}\\s*(?:\\.\\s*(?:${RECEIVER_ONLY_NAMES.join('|')})\\s*\\(|\\[\\s*(['"\`])(?:${FS_SHORT_NAMES.join('|')})\\1\\s*\\])`,
      'g',
    );
    while ((m = member.exec(text)) !== null) lines.add(lineOf(m.index));
  }
  return lines;
}

// [#576 RV8b-E3-3] The unmocked-IPC teardown guard (E3) is an auto fixture on the
// `test` that e2e-playwright/fixtures/coverage.ts exports: a spec that takes `test`
// from anywhere else runs with no guard, and an unmocked command silently answers
// null. So a spec must import `test` from fixtures/coverage, and must take no value
// import of `test` (nor a default / namespace import) from '@playwright/test' - a
// type-only import is fine. Returns the 0-based lines to report.
const SPEC_FILE_RE = /\.spec\.ts$/;
// The module may be named with its extension, `./fixtures/coverage.js` (#576 RV9b-7).
const GUARDED_TEST_IMPORT_RE =
  /\bimport\s*\{[^}]*\btest\b[^}]*\}\s*from\s*(['"`])(?:\.\.?\/)+(?:[\w.-]+\/)*fixtures\/coverage(?:\.[cm]?[jt]s)?\1/;
const PLAYWRIGHT_IMPORT_RE = /\bimport\s+(type\s+)?([^;]*?)\s*from\s*(['"`])@playwright\/test\3/g;
function unguardedTestLines(code, rel) {
  const lines = new Set();
  if (!SPEC_FILE_RE.test(rel)) return lines;
  const text = code.join('\n');
  const lineOf = (at) => text.slice(0, at).split('\n').length - 1;
  PLAYWRIGHT_IMPORT_RE.lastIndex = 0;
  let m;
  while ((m = PLAYWRIGHT_IMPORT_RE.exec(text)) !== null) {
    if (m[1]) continue; // `import type { ... }`
    const named = /\{([^}]*)\}/.exec(m[2]);
    const valueTest = named !== null && named[1].split(',').some((spec) => /^\s*test\b/.test(spec));
    const defaultOrNamespace = /^\s*(?:\*\s*as\s+[\w$]+|[\w$]+)\s*(?:,|$)/.test(m[2]);
    if (valueTest || defaultOrNamespace) lines.add(lineOf(m.index));
  }
  if (!GUARDED_TEST_IMPORT_RE.test(text)) lines.add(Math.max(0, code.findIndex((l) => l.trim())));
  return lines;
}

// One rule per parallel-unsafe pattern. `re` is tested per comment-stripped line;
// an optional `accept(match)` exempts a benign sub-case (used for the shared port),
// and an optional `fileLines(code, rel)` adds the lines a whole-file pattern fires on.
export const RULES = [
  {
    id: 'own-server',
    // A WebSocket server binds a port just the same (`ws`: `new WebSocketServer(`,
    // or the older `new WebSocket.Server(`; #576 NF-FIX3B-1).
    re: /\bcreateServer\s*\(|\bnew\s+(?:WebSocketServer|WebSocket\.Server)\s*\(/,
    why: 'starts its own server - every page shares the one Vite webServer (:1420).',
  },
  {
    id: 'port-listen',
    re: /\.listen\s*\(/,
    why: 'binds a port - parallel workers would collide on it.',
  },
  {
    id: 'disk-write',
    // Require the call paren so mock command labels such as 'plugin:fs|writeFile'
    // (string literals, no `(`) do not false-positive. `page.screenshot({path})`
    // is intentionally absent - it's framework I/O, used here with unique names.
    // Creating, deleting and linking paths mutate shared disk state exactly as a
    // write does (#576 E7). The ambiguous short async names (rm, cp, rename,
    // truncate) only count as a member call (`fs.rm(`, `fsp.rename(`), so a UI
    // helper called `rename(` does not fire. mkdtemp is not here: its path is
    // unique by construction. Opening a file for writing (openSync + writeSync),
    // a hard link and a mode change are the same shared-disk mutation
    // (#576 RV3-FE-12); `open` / `link` / `chmod` are common method names
    // (`window.open(`, a menu's `open(`), so their async forms count only on an
    // fs-looking receiver (`fs.`, `fsp.`, `fsPromises.`, `promises.`).
    // Bracket access (`fs['writeFileSync'](p)`; the short names only on an fs
    // receiver) and a name imported or destructured from fs / fs/promises,
    // alias or not (`import { writeFileSync as w } from 'node:fs'`,
    // `const { open } = fs.promises`), count too (#576 RV4-FE-FEB-3), as does
    // one destructured through `promises` (`const { promises: { open } } = fs`,
    // #576 NF-W9FE-4), under a quoted or computed key or a typed pattern
    // (`const { ['open']: o } = fs.promises`, `}: typeof fs =`), and a call on
    // a new name for the module (`const { promises: p } = fs; p.open(...)`,
    // `import * as nodeFs from 'fs'`, `const P = fs.promises`; #576 RV10b-G6);
    // the import check reads the whole file, so a multi-line brace counts. Not
    // seen: a `}` inside a string default within the pattern.
    re: new RegExp(
      `\\b(?:${LONG})\\s*\\(|\\.(?:rm|cp|rename|truncate)\\s*\\(|${FS_RECEIVER}\\.(?:open|link|chmod)\\s*\\(`
        + `|\\[\\s*(['"\`])(?:${LONG})\\1\\s*\\]|${FS_RECEIVER}\\s*\\[\\s*(['"\`])(?:${FS_SHORT_NAMES.join('|')})\\2\\s*\\]`,
    ),
    fileLines: fsImportLines,
    why: 'writes, creates or deletes on disk - use a per-(test,worker)-unique path only (see fixtures/coverage.ts), else concurrent workers clobber each other.',
  },
  {
    id: 'foreign-host-port',
    // A literal port, a template-interpolated one (`localhost:${port}`), or one
    // concatenated onto the string (`'localhost:' + port`), which is how a
    // per-test server's address is usually spelled; `0.0.0.0` is the
    // all-interfaces spelling of the same bind (#576 NF-FIX3B-1), and `[::1]` /
    // `[::]` are the IPv6 loopback and all-interfaces ones (#576 NF-W4M-5).
    re: /(?:127\.0\.0\.1|localhost|0\.0\.0\.0|\[::1?\]):(?:(\d+|\$\{[^}]*\})|['"`]\s*\+\s*([^\s;,)]+))/,
    // Allow the shared Vite port, in any of the three spellings.
    accept: (m) => (m[1] ?? m[2]).replace(/^['"`]|['"`]$/g, '') === SHARED_VITE_PORT,
    why: `hardcodes a host:port other than :${SHARED_VITE_PORT} - implies a per-test server/socket, not parallel-safe.`,
  },
  {
    // Not a parallel-safety rule: the one tier invariant this walk over every spec
    // also checks (#576 RV8b-E3-3, see unguardedTestLines). Whole-file only.
    id: 'unguarded-test',
    re: /(?!)/,
    fileLines: unguardedTestLines,
    why: "takes `test` from somewhere other than ./fixtures/coverage - the unmocked-IPC teardown guard (#576 E3) rides on that fixture's `test`; import { test } from './fixtures/coverage' (a type-only import from @playwright/test is fine).",
  },
  {
    id: 'serial',
    // Both the `test.describe.serial(...)` shorthand and the idiomatic
    // `test.describe.configure({ mode: 'serial' })` form.
    re: /\b(?:describe|test)\.serial\b|mode:\s*['"]serial['"]/,
    why: 'serial execution defeats fullyParallel. If genuinely required, add this file to ALLOW in this script with a written reason.',
  },
];

export function isAllowed(relFile, ruleId) {
  return ALLOW.some((a) => a.file === relFile && a.rule === ruleId);
}

// Comment stripping is the shared string-aware scanner in
// scripts/lib/js-comments.mjs (re-exported for the fixture test). The old
// per-line version here opened a block comment at ANY `/*` - including one
// inside a string such as 'fixtures/*.dmp' - and then blanked the rest of the file.
export { stripComments };

// Scan files (repo-relative paths). `readFile(rel)` returns text; injected so
// the fixture test drives every rule without a tree on disk.
export function scan(files, readFile) {
  const violations = [];
  for (const rel of files) {
    const lines = readFile(rel).split('\n');
    const code = stripComments(lines);
    const fileHits = new Map(RULES.filter((r) => r.fileLines).map((r) => [r.id, r.fileLines(code, rel)]));
    code.forEach((c, i) => {
      if (!c.trim()) return;
      for (const rule of RULES) {
        const m = rule.re.exec(c);
        if (!m && !fileHits.get(rule.id)?.has(i)) continue;
        if (m && rule.accept && rule.accept(m)) continue;
        if (isAllowed(rel, rule.id)) continue;
        violations.push({ rel, lineNo: i + 1, ruleId: rule.id, why: rule.why, snippet: lines[i].trim() });
      }
    });
  }
  return violations;
}

// The premise every rule rests on. Returns a problem string, or null.
export function checkPremise(configText) {
  if (configText === null) return `${CONFIG_FILE} not found - cannot confirm the suite runs fullyParallel.`;
  const code = stripComments(configText.split('\n')).join('\n');
  if (!/\bfullyParallel\s*:\s*true\b/.test(code)) {
    return `${CONFIG_FILE} no longer sets \`fullyParallel: true\` - the rules below guard that mode; if it was turned off on purpose, say why here and in docs/testing-recipes.md.`;
  }
  return null;
}

export function main(root = REPO_ROOT, log = console.log, err = console.error) {
  if (!fs.existsSync(path.join(root, SCAN_DIR))) {
    err(`check-e2e-parallel-safe: ${SCAN_DIR}/ not found (run from the repo root).`);
    return 1;
  }
  const { files, symlinks } = walkFiles(root, SCAN_DIR, { exts: ['.ts', '.tsx'] });
  const violations = scan(files, (rel) => fs.readFileSync(path.join(root, rel), 'utf8'));
  for (const link of symlinks) {
    violations.push({ rel: link.rel, lineNo: 0, ruleId: 'symlink', why: 'a symlink is not followed, so its target is unscanned - replace it with a regular file.', snippet: `-> ${link.target}` });
  }
  const configPath = path.join(root, CONFIG_FILE);
  const premise = checkPremise(fs.existsSync(configPath) ? fs.readFileSync(configPath, 'utf8') : null);
  if (premise) violations.push({ rel: CONFIG_FILE, lineNo: 0, ruleId: 'premise', why: premise, snippet: '' });

  if (violations.length === 0) {
    log(`✓ e2e-playwright parallel-safe: scanned ${files.length} files, no unsafe patterns (${CONFIG_FILE}: fullyParallel).`);
    return 0;
  }

  err(`\n✗ e2e-playwright parallel-safety check failed - ${violations.length} issue(s):\n`);
  for (const v of violations) {
    err(`  [${v.ruleId}] ${v.rel}${v.lineNo ? `:${v.lineNo}` : ''}`);
    if (v.snippet) err(`      ${v.snippet}`);
    err(`      ${v.why}\n`);
  }
  err(
    'The mocked Playwright tier runs fullyParallel at multiple workers; all state must\n' +
      'stay per-page (window.*). See docs/testing-recipes.md, "Parallel-safety invariant".\n',
  );
  return 1;
}

const isMainModule =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMainModule) process.exit(main());
