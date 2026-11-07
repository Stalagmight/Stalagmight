// Shared-IPC-mock gate for the vitest tier (#576 E15-NF2).
//
// #576 E15 replaced ~120 hand-copied fakes of `src/utils/ipc` (a
// `vi.hoisted(() => vi.fn())` plus a `vi.mock('<rel>/utils/ipc', () => ({
// invokeIpc: ..., getErrorMessage: ... }))` factory, twelve getErrorMessage
// variants among them) with one manual mock, `src/utils/__mocks__/ipc.ts`,
// driven through `src/test/factories/ipc.ts`. Nothing stopped the next test
// from copying the old idiom again, and six did within one wave. This gate
// fails on every `vi.mock` / `vi.doMock` of `src/utils/ipc` that passes a
// second argument - an inline factory, a `vi.hoisted` factory, an
// `() => import(...)` module - outside ALLOWLIST. Zero tolerance, no baseline.
//
// The idiom to use instead (docs/testing-recipes.md, "Factories and the shared
// IPC mock"):
//
//     import { invokeIpcMock } from '<rel>/test/factories/ipc';
//     vi.mock('<rel>/utils/ipc');
//
// What counts: `vi.mock(` / `vi.doMock(` (whitespace and line breaks allowed)
// whose first argument is a string literal - or `import('<spec>')`, the typed
// form - naming src/utils/ipc: relative to the file (`'../ipc'`,
// `'../../utils/ipc.ts'`) or root-relative (`'/src/utils/ipc'`), and whose
// argument list goes on past it. Comments are stripped with the shared
// string-aware scanner, so a factory in prose does not count; a mock of any
// other module (`'../ipcEvents'`, `'@tauri-apps/api/core'`) is out of scope.
//
// ALLOWLIST: the files #576 E15 kept on purpose, each with its reason. A row
// whose file is gone or no longer carries a factory fails the gate, so the list
// cannot outlive its reason. A symlinked source file fails, as in every
// source gate.
//
// Runs in the `check` job (ci.yml) after its fixture test
// (scripts/check-vitest-ipc-mock.test.mjs).
//
// Usage:  node scripts/check-vitest-ipc-mock.mjs   (or: pnpm lint:vitest-ipc-mock)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { walkFiles, symlinkProblem } from './lib/src-files.mjs';
import { matchParen, stripComments } from './lib/js-comments.mjs';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// Every tree vitest collects tests from (vite.config.ts `test.include`).
export const SCAN_DIRS = ['src', 'scripts', 'e2e/agent/lib', 'e2e/agent/os', 'e2e/lib'];
export const EXTS = ['.ts', '.tsx', '.mts', '.cts'];
export const IPC_MODULE = 'src/utils/ipc';

export const ALLOWLIST = {
  'src/hooks/__tests__/useInternalErrorEvent.pull.test.tsx':
    'its invokeIpc must be a plain function, not a vi.fn: a vi.fn marks the rejection it returns as ' +
    'handled, which hides the unhandled rejection this test is about (#576 E15)',
  'src/store/__tests__/storeLoadOrder.test.ts':
    'vi.resetModules() re-evaluates a factory-less __mocks__ module, so the stores it re-imports would ' +
    'get a fresh invokeIpc; the `() => import(<rel>/utils/__mocks__/ipc)` factory keeps one (#576 E15)',
  'src/test/factories/__tests__/ipcMockResetModules.test.ts':
    'pins the storeLoadOrder idiom above: one invokeIpc across vi.resetModules() (#576 E15)',
};

const CALL_RE = /\bvi\s*\.\s*(?:mock|doMock)\s*\(/g;
// Same-line string literals, blanked (length kept) before calls are looked for,
// so a vi.mock quoted in a string is not a call.
const STRING_RE = /'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"|`(?:\\.|[^`\\])*`/g;
const blank = (lit) => lit[0] + ' '.repeat(lit.length - 2) + lit[lit.length - 1];
// The first argument: a string literal, or `import('<spec>')`.
const FIRST_ARG_RE = /^\s*(?:import\s*\(\s*)?(['"`])([^'"`$]+)\1\s*(\)\s*)?/;

// The repo-relative module a specifier names from `rel`, without extension or
// `/index`; null for a bare package name.
export function resolveSpecifier(rel, spec) {
  let p;
  if (spec.startsWith('.')) p = path.posix.normalize(path.posix.join(path.posix.dirname(rel), spec));
  else if (spec.startsWith('/')) p = path.posix.normalize(spec.slice(1));
  else return null;
  return p.replace(/\.(?:[cm]?[jt]sx?)$/, '').replace(/\/index$/, '');
}

// Every vi.mock / vi.doMock of src/utils/ipc in one file that passes a factory:
// [{lineNo}] (1-based line of the `vi`).
export function findFactoryMocks(text, rel) {
  const stripped = stripComments(text.split('\n'));
  const code = stripped.join('\n');
  const calls = stripped.map((line) => line.replace(STRING_RE, blank)).join('\n');
  const hits = [];
  let m;
  CALL_RE.lastIndex = 0;
  while ((m = CALL_RE.exec(calls)) !== null) {
    const open = m.index + m[0].length - 1;
    const close = matchParen(code, open);
    const args = code.slice(open + 1, close - 1);
    const first = FIRST_ARG_RE.exec(args);
    if (!first) continue;
    // `import('x')` must close its own paren; a bare literal must not have one.
    const isImport = /^\s*import\s*\(/.test(args);
    if (isImport !== Boolean(first[3])) continue;
    if (resolveSpecifier(rel, first[2]) !== IPC_MODULE) continue;
    const rest = args.slice(first[0].length);
    if (/^,\s*\S/.test(rest)) hits.push({ lineNo: code.slice(0, m.index).split('\n').length });
  }
  return hits;
}

// Scan repo-relative files; `readFile` is injected for the fixture test.
export function scan(files, readFile, allowlist = ALLOWLIST) {
  const violations = [];
  const allowed = new Set();
  for (const rel of files) {
    const hits = findFactoryMocks(readFile(rel), rel);
    if (hits.length === 0) continue;
    if (Object.prototype.hasOwnProperty.call(allowlist, rel)) {
      allowed.add(rel);
      continue;
    }
    for (const h of hits) violations.push({ rel, lineNo: h.lineNo });
  }
  const stale = Object.keys(allowlist).filter((rel) => !allowed.has(rel));
  return { violations, stale, allowed: [...allowed] };
}

export function main(root = REPO_ROOT, log = console.log, err = console.error) {
  if (!fs.existsSync(path.join(root, 'src'))) {
    err('check-vitest-ipc-mock: src/ not found (run from the repo root).');
    return 1;
  }
  const files = [];
  const symlinks = [];
  for (const dir of SCAN_DIRS) {
    if (!fs.existsSync(path.join(root, dir))) continue;
    const found = walkFiles(root, dir, { exts: EXTS, skip: (rel) => /(^|\/)target$/.test(rel) });
    files.push(...found.files);
    symlinks.push(...found.symlinks);
  }
  const { violations, stale, allowed } = scan(files, (rel) => fs.readFileSync(path.join(root, rel), 'utf8'));
  if (violations.length === 0 && stale.length === 0 && symlinks.length === 0) {
    log(`✓ vitest utils/ipc mock: ${files.length} files, no factory mock of utils/ipc outside the allowlist (${allowed.length} allowlisted).`);
    return 0;
  }
  err(`\n✗ vitest utils/ipc mock check failed - ${violations.length + stale.length + symlinks.length} issue(s):\n`);
  for (const v of violations) err(`  ${v.rel}:${v.lineNo}  vi.mock of utils/ipc with a factory (the copied vi.hoisted + factory idiom)`);
  for (const rel of stale) err(`  ${rel}  is in ALLOWLIST but has no factory mock of utils/ipc (or is gone) - drop the row`);
  for (const l of symlinks) err(`  ${symlinkProblem(l)}`);
  err(
    '\nUse the shared mock: `vi.mock(\'<rel>/utils/ipc\')` with no factory, and script\n' +
      '`invokeIpcMock` from src/test/factories/ipc.ts (getErrorMessage: stubGetErrorMessage).\n' +
      'See docs/testing-recipes.md, "Factories and the shared IPC mock". A file that truly\n' +
      'needs its own factory goes in ALLOWLIST in scripts/check-vitest-ipc-mock.mjs, with its reason.\n',
  );
  return 1;
}

const isMainModule = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMainModule) process.exit(main());
