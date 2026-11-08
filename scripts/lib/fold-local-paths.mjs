// The one implementation of foldLocalPaths (#576 RV18-W23-4, W58): the agent
// harness imports it through e2e/agent/lib/shareableFailure.ts, and the repo's
// Node tools under scripts/ import it here, so a CLI or env path they echo
// prints folded instead of as a placeholder. The print-site guard knows the
// name as a fold (e2e/agent/lib/printSites/rules.ts FOLDS). execFileSyncFolded
// runs a developer tool (git, cargo, pnpm) with its stderr folded the same way.

import { spawnSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'url';

/** The checkout: this file is scripts/lib/fold-local-paths.mjs. */
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

/**
 * The temp directory as `os.tmpdir()` gives it, and its real path when that
 * differs (macOS hands out `/var/folders/...`, which is `/private/var/...`).
 */
function tempRoots() {
  const tmp = os.tmpdir();
  try {
    const real = fs.realpathSync(tmp);
    return real === tmp ? [tmp] : [tmp, real];
  } catch {
    return [tmp];
  }
}

/**
 * `text` with the checkout folded to `<repo>`, the home directory to `~`,
 * `CARGO_TARGET_DIR` to `<target>` and the temp directory to `<tmp>`. A
 * shareable line can carry a path - a `reproduce:` line's fixture or `REPLAY
 * of <trace>`, a headline naming the session directory, a build's `Built
 * application at:` line, a save under the temp directory, a path a tool was
 * given on its command line - and a path under any of these roots names the
 * user or their machine. What follows the folded root is the harness's own
 * layout, or a corpus id the corpus fold already put there. Read at call time,
 * so a changed `HOME`, `CARGO_TARGET_DIR` or `TMPDIR` is honoured; the longest
 * root folds first, and a root of `/` or a relative `CARGO_TARGET_DIR` folds
 * nothing.
 *
 * @param {string} text
 * @returns {string}
 */
export function foldLocalPaths(text) {
  const target = process.env.CARGO_TARGET_DIR;
  const roots = [
    { root: REPO_ROOT, say: '<repo>' },
    { root: os.homedir(), say: '~' },
    ...(target ? [{ root: target, say: '<target>' }] : []),
    ...tempRoots().map((root) => ({ root, say: '<tmp>' })),
  ]
    .filter(({ root }) => path.isAbsolute(root) && path.resolve(root) !== path.parse(root).root)
    .map(({ root, say }) => ({ root: path.resolve(root), say }))
    .sort((a, b) => b.root.length - a.root.length);
  let out = text;
  for (const { root, say } of roots) {
    if (!out.includes(root)) continue;
    const literal = root.replace(/[.*+?^$()|[\]{}\\]/g, '\\$&');
    // Not inside a longer name: `/home/bob` must leave `/home/bobby` alone.
    out = out.replace(new RegExp(`${literal}(?![\\w.-])`, 'g'), say);
  }
  return out;
}

/**
 * `execFileSync(file, args, options)` with the child's stderr captured and
 * written to this process's stderr through foldLocalPaths (#576 W58 Q6). A
 * gate's `git ls-files` or the license generator's cargo / pnpm call can name
 * the checkout in its diagnostics (git's `detected dubious ownership in
 * repository at '<path>'`, cargo's `failed to parse manifest at <path>`), which
 * execFileSync's default stderr passes through as it is.
 *
 * Otherwise it is execFileSync: it returns the child's stdout (a Buffer, or a
 * string with `encoding`), writes the stderr before it returns or throws, and
 * throws what execFileSync throws - the spawn error (`spawnSync git ENOENT`,
 * ENOBUFS past `maxBuffer`) with the result's fields on it, or for a non-zero
 * exit or a signal an Error whose first line is `Command failed: <file>
 * <args>`, followed by the stderr, with `status`, `signal`, `stdout` and
 * `stderr` on it. Only its message is folded too; `stderr` stays the child's
 * own output.
 *
 * @param {string} file
 * @param {readonly string[]} [args]
 * @param {import('child_process').SpawnSyncOptions} [options] any `stdio` is replaced by pipes
 * @returns {string | Buffer}
 */
export function execFileSyncFolded(file, args = [], options = {}) {
  const ret = spawnSync(file, args, { ...options, stdio: 'pipe' });
  const stderr = ret.stderr == null ? '' : String(ret.stderr);
  if (stderr.length > 0) process.stderr.write(foldLocalPaths(stderr));
  if (ret.error) throw Object.assign(ret.error, ret);
  if (ret.status !== 0) {
    const message = foldLocalPaths(`Command failed: ${[file, ...args].join(' ')}${stderr.length > 0 ? `\n${stderr}` : ''}`);
    throw Object.assign(new Error(message), ret);
  }
  return ret.stdout;
}
