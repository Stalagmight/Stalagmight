// Shared source-tree discovery for the per-file gates (#576 E16):
// check-test-ids.mjs, check-ts-file-size.mjs, the read-only gating tripwire,
// and check-e2e-parallel-safe.mjs.
//
// Those gates used three different discovery mechanisms with three exclusion
// rulesets: one listed `git ls-files` (so an unstaged file dodged it), one
// walked the filesystem and silently skipped symlinks, one walked it and
// FOLLOWED a symlinked file. The single policy now:
//   - a filesystem walk, so an untracked file cannot dodge a gate by never
//     being staged (the file-size ratchets' original rationale);
//   - `node_modules` is never descended into;
//   - a symlink is never followed and never silently skipped: it is returned
//     in `symlinks`, and each gate FAILS on it (a gate cannot vouch for content
//     it did not read, and a symlink under src/ has no legitimate use today);
//   - one test/generated predicate pair, so "production file" means the same
//     thing in every gate.

import * as fs from 'fs';
import * as path from 'path';

import { foldLocalPaths } from './fold-local-paths.mjs';

// Test code: a __tests__/ directory, a *.test.* / *.spec.* file, or the
// src/test/ harness (vitest setup and its own tests).
const TEST_DIR_RE = /(^|\/)__tests__(\/|$)/;
const TEST_NAME_RE = /\.(test|spec)\.[cm]?[jt]sx?$/;
const TEST_HARNESS_RE = /^src\/test\//;

export function isTestPath(rel) {
  return TEST_DIR_RE.test(rel) || TEST_NAME_RE.test(rel) || TEST_HARNESS_RE.test(rel);
}

// ts-rs output (CLAUDE.md "IPC Types Are Generated"): never hand-written.
export function isGeneratedPath(rel) {
  return rel.startsWith('src/types/ipc/') || rel === 'src/types/ipc.generated.ts';
}

const relPosix = (root, full) => path.relative(root, full).split(path.sep).join('/');

// Walk `<root>/<dir>` and return {files, symlinks} as repo-relative POSIX
// paths, sorted. `exts` filters regular files by suffix; `skip(rel)` prunes a
// directory or file before it is listed (generated replays, fixtures, ...).
// Throws if `dir` does not exist - callers decide whether that fails the gate.
export function walkFiles(root, dir, { exts, skip = () => false } = {}) {
  const files = [];
  const symlinks = [];
  const visit = (full) => {
    const entries = fs.readdirSync(full, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
    for (const e of entries) {
      const child = path.join(full, e.name);
      const rel = relPosix(root, child);
      if (e.name === 'node_modules' || skip(rel)) continue;
      if (e.isSymbolicLink()) {
        let target = '?';
        try {
          target = fs.readlinkSync(child);
        } catch {
          // keep '?': the link is reported either way
        }
        // Every gate prints the target. One that is absolute or leaves the root
        // names a path on this machine - the home directory, the user's own
        // folders - not the checkout's text, so it prints resolved and folded
        // (#576 W51, W58).
        const absolute = path.resolve(path.dirname(child), target);
        const resolved = path.relative(root, absolute);
        const leaves = path.isAbsolute(target) || path.isAbsolute(resolved) || resolved === '..' || resolved.startsWith(`..${path.sep}`);
        symlinks.push({ rel, target: leaves ? foldLocalPaths(absolute) : target });
      } else if (e.isDirectory()) {
        visit(child);
      } else if (e.isFile() && (!exts || exts.some((x) => e.name.endsWith(x)))) {
        files.push(rel);
      }
    }
  };
  visit(path.join(root, dir));
  return { files, symlinks };
}

// Production TS/TSX under src/: tests and generated IPC types excluded.
export function listSrcFiles(root, { exts = ['.ts', '.tsx'] } = {}) {
  return walkFiles(root, 'src', { exts, skip: (rel) => isTestPath(rel) || isGeneratedPath(rel) });
}

// One message for every gate, so the same symlink reads the same everywhere.
export function symlinkProblem(link) {
  return `${link.rel} is a symlink (-> ${link.target}); source gates do not follow links - replace it with a regular file`;
}
