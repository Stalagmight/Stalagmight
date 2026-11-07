/**
 * Build + run the Tier 2 OS helper (`os/helper/`, the `stalagmight-oshelper`
 * Rust bin crate). See ./README.md for what Tier 2 is and who grants what.
 *
 * Freshness mirrors `lib/build.ts`: an mtime comparison against the crate's
 * sources, plus a stamp beside the binary recording the mtime the build
 * produced. The stamp is what catches a binary rebuilt behind our back (a hand
 * `cargo build` in the crate dir, a `--release` run with different flags);
 * mtimes alone would call that fresh.
 *
 * The helper binary shares the harness cargo target dir with the app binary -
 * different package names, no collision - so a contributor already carrying
 * `~/.cargo_build` does not grow a second one.
 */
import { execFile, spawn } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { cargoTargetDir } from '../../lib/env';
import { passLinesFolded } from '../lib/foldedLines';

const osDir = path.dirname(fileURLToPath(import.meta.url));

/** Directory of the helper crate (the cargo working directory). */
export const helperCrateDir = path.join(osDir, 'helper');
/** Absolute path of the built release binary. */
export const helperBinaryPath = path.join(cargoTargetDir, 'release', 'stalagmight-oshelper');

const stampPath = path.join(cargoTargetDir, 'release', '.stalagmight-oshelper-stamp.json');
/** Inputs whose change invalidates the built helper. */
const SOURCE_DIR = path.join(helperCrateDir, 'src');
const SOURCE_FILES = [
  path.join(helperCrateDir, 'Cargo.toml'),
  path.join(helperCrateDir, 'Cargo.lock'),
];

/** Default wall-clock budget for one helper run. */
export const DEFAULT_HELPER_TIMEOUT_MS = 10_000;

export interface HelperBuildStamp {
  /** Stamp schema version; an unknown version is treated as no stamp. */
  v: 1;
  /** mtime (ms) of the binary right after the build that wrote this stamp. */
  binaryMtimeMs: number;
  builtAtMs: number;
  builtAtIso: string;
}

function mtimeOf(file: string): number {
  try {
    return fs.statSync(file).mtimeMs;
  } catch {
    return 0;
  }
}

/** Newest mtime (ms) among files under `dir`, recursively; 0 if missing. */
function newestMtimeInDir(dir: string): number {
  let newest = 0;
  let entries: fs.Dirent[];
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return 0;
  }
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    newest = Math.max(newest, entry.isDirectory() ? newestMtimeInDir(full) : mtimeOf(full));
  }
  return newest;
}

/** Read + validate the stamp. Returns null if missing, unreadable, or bogus. */
export function readHelperStamp(file: string = stampPath): HelperBuildStamp | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(fs.readFileSync(file, 'utf-8'));
  } catch {
    return null;
  }
  if (typeof parsed !== 'object' || parsed === null) return null;
  const candidate = parsed as Partial<HelperBuildStamp>;
  if (candidate.v !== 1) return null;
  if (typeof candidate.binaryMtimeMs !== 'number') return null;
  return {
    v: 1,
    binaryMtimeMs: candidate.binaryMtimeMs,
    builtAtMs: typeof candidate.builtAtMs === 'number' ? candidate.builtAtMs : 0,
    builtAtIso: typeof candidate.builtAtIso === 'string' ? candidate.builtAtIso : '',
  };
}

/**
 * Why the current helper binary cannot be trusted. Empty means it is fresh.
 * Pure over the numbers so the decision is inspectable without a build.
 */
export function helperStaleReasons(input: {
  binaryMtimeMs: number;
  newestSourceMtimeMs: number;
  stamp: HelperBuildStamp | null;
}): string[] {
  if (input.binaryMtimeMs === 0) return [`helper binary missing at ${helperBinaryPath}`];
  const reasons: string[] = [];
  if (!input.stamp) {
    reasons.push('no helper build stamp: the binary was not built by this harness');
  } else if (input.stamp.binaryMtimeMs !== input.binaryMtimeMs) {
    reasons.push('the helper binary was rebuilt outside the harness since its build stamp');
  }
  if (input.newestSourceMtimeMs > input.binaryMtimeMs) {
    reasons.push('a helper source file is newer than the binary');
  }
  return reasons;
}

/** Freshness of the helper binary on disk. */
export function checkHelperFreshness(): { fresh: boolean; reasons: string[] } {
  const newestSourceMtimeMs = Math.max(
    newestMtimeInDir(SOURCE_DIR),
    ...SOURCE_FILES.map(mtimeOf),
  );
  const reasons = helperStaleReasons({
    binaryMtimeMs: mtimeOf(helperBinaryPath),
    newestSourceMtimeMs,
    stamp: readHelperStamp(),
  });
  return { fresh: reasons.length === 0, reasons };
}

/**
 * Build the helper if it is missing or stale. A one-time cold cost (a handful
 * of small crates), and every os_* action needs it done before it can do
 * anything at all, so `runHelper` awaits it. The MCP server runs one tool
 * handler at a time, so two builds do not race.
 */
export async function ensureHelperBuilt(onLog?: (line: string) => void): Promise<string> {
  const { fresh, reasons } = checkHelperFreshness();
  if (fresh) return helperBinaryPath;
  onLog?.(`Building stalagmight-oshelper (${reasons.join('; ')})...`);
  await buildHelper();
  writeHelperStamp(onLog);
  onLog?.('Helper build complete.');
  return helperBinaryPath;
}

/**
 * `cargo build --release` in the helper crate, its stdout and stderr both passed
 * to this process's stderr a line at a time with the checkout folded to `<repo>`
 * and the home directory to `~` (`passLinesFolded`, #576 W51), as `lib/build.ts`
 * passes the app build's. Inherited, cargo's `Compiling stalagmight-oshelper
 * (<path>)` named the checkout beside the server's other lines. NEVER this
 * process's stdout: under the MCP server stdout is the JSON-RPC stream and cargo
 * chatter would corrupt the protocol framing. Rejects with `Command failed:
 * cargo build --release`, as the `execFileSync` it replaces threw, when the
 * build exits non-zero or dies.
 */
function buildHelper(): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn('cargo', ['build', '--release'], {
      cwd: helperCrateDir,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, CARGO_TARGET_DIR: cargoTargetDir },
    });
    const write = (text: string): void => {
      process.stderr.write(text);
    };
    const passed = Promise.all([passLinesFolded(child.stdout, write), passLinesFolded(child.stderr, write)]);
    child.once('error', reject);
    child.once('close', (code: number | null) => {
      void passed.then(() => (code === 0 ? resolve() : reject(new Error('Command failed: cargo build --release'))));
    });
  });
}

/**
 * Record what the build produced. Best-effort: a stamp we fail to write just
 * means the next call rebuilds.
 */
function writeHelperStamp(onLog?: (line: string) => void): void {
  const now = Date.now();
  const stamp: HelperBuildStamp = {
    v: 1,
    binaryMtimeMs: mtimeOf(helperBinaryPath),
    builtAtMs: now,
    builtAtIso: new Date(now).toISOString(),
  };
  try {
    fs.mkdirSync(path.dirname(stampPath), { recursive: true });
    fs.writeFileSync(stampPath, `${JSON.stringify(stamp, null, 2)}\n`);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    onLog?.(`Warning: could not write the helper build stamp (${msg}); it will rebuild.`);
  }
}

/** The helper's failure envelope. A legitimate result, not an exception. */
export interface HelperError {
  ok: false;
  error: string;
}

export interface PreflightResult {
  ok: true;
  accessibility: boolean;
  screenRecording: boolean;
}

export interface MoveResult {
  ok: true;
  action: 'move';
  x: number;
  y: number;
}

export interface ClickResult {
  ok: true;
  action: 'click';
  x: number;
  y: number;
  button: 'left' | 'right';
  clickCount: number;
}

export interface DragResult {
  ok: true;
  action: 'drag';
  from: { x: number; y: number };
  to: { x: number; y: number };
  durationMs: number;
  steps: number;
}

export interface KeysResult {
  ok: true;
  action: 'keys';
  spec: string;
  key: string;
  keyCode: number;
  modifiers: string[];
}

export interface WindowResult {
  ok: true;
  windowId: number;
  pid: number;
  app: string;
  /** Screen POINTS, top-left origin - the same space clicks are posted in. */
  bounds: { x: number; y: number; w: number; h: number };
}

export interface FrontmostResult {
  ok: true;
  pid: number;
  app: string;
  windowId: number;
}

export type HelperSuccess =
  | PreflightResult
  | MoveResult
  | ClickResult
  | DragResult
  | KeysResult
  | WindowResult
  | FrontmostResult;

export type HelperResult = HelperSuccess | HelperError;

export interface RunHelperOptions {
  /** Wall-clock budget. Must exceed a drag's own `--duration-ms`. */
  timeoutMs?: number;
  /** Build (or rebuild) the helper first. Default true. */
  ensureBuilt?: boolean;
}

/**
 * Run the helper and parse its single JSON line.
 *
 * A helper-level failure (`{ok:false,...}`, exit 1) is RETURNED, not thrown:
 * "Accessibility is denied" and "no window for that pid" are results the
 * caller must surface, and modeling them as exceptions loses the message.
 * Only an infrastructure fault - cannot spawn, timed out, output that is not
 * JSON - throws, because at that point nothing is known about the machine.
 *
 * Async on purpose: the MCP server must stay responsive while a drag plays out
 * over its interpolation window.
 */
export async function runHelper<T extends HelperResult = HelperResult>(
  args: string[],
  options: RunHelperOptions = {},
): Promise<T> {
  const timeout = options.timeoutMs ?? DEFAULT_HELPER_TIMEOUT_MS;
  const binary = options.ensureBuilt === false ? helperBinaryPath : await ensureHelperBuilt();

  const { stdout, stderr, failure } = await new Promise<{
    stdout: string;
    stderr: string;
    /** Spawn failure, nonzero exit, or the timeout kill - message only. */
    failure: Error | null;
  }>((resolve) => {
    execFile(
      binary,
      args,
      { timeout, encoding: 'utf-8', killSignal: 'SIGKILL' },
      (error, out, errOut) => resolve({ stdout: out, stderr: errOut, failure: error }),
    );
  });

  const trimmed = stdout.trim();
  if (trimmed !== '') {
    try {
      return JSON.parse(trimmed) as T;
    } catch {
      // Fall through: unparseable output is an infrastructure fault, and the
      // raw text is the only diagnostic there is.
    }
  }
  const detail = [failure?.message, stderr.trim(), trimmed && `stdout: ${trimmed}`]
    .filter(Boolean)
    .join(' | ');
  throw new Error(
    `stalagmight-oshelper ${args.join(' ')} produced no JSON result` +
      `${detail ? ` (${detail})` : ''}`,
  );
}
