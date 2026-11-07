/**
 * Build freshness + build for the agent harness. Mirrors the intent of
 * the retired scripts/wdio.sh's freshness check: the debug binary must be newer than the
 * frontend/backend sources and must have been built with VITE_E2E=1 (the
 * window.__E2E_TEST__ hooks).
 *
 * mtimes alone cannot see *how* the binary was built, so a binary produced by
 * `./scripts/dev.sh` or a plain `pnpm tauri build` (no VITE_E2E) used to look
 * "fresh" and `build: "auto"` attached to a hookless dist, only to fail the
 * post-attach `__E2E_TEST__` check (issue #260). `buildApp` therefore drops an
 * E2E build stamp next to the binary recording that the build ran with
 * VITE_E2E=1, plus the mtime of the binary it produced. A missing stamp, a
 * stamp that disagrees with what this start is asking for, or a binary whose
 * mtime no longer matches the stamp (something rebuilt it behind our back) all
 * count as stale, so `auto` rebuilds *before* attach instead of failing after.
 *
 * The post-attach `__E2E_TEST__` check in session.ts stays as the definitive
 * backstop; this is the cheap pre-flight.
 */
import { execFileSync, execSync, spawn } from 'child_process';
import { createHash } from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { agentBuildMode, binaryPathFor, cargoTargetDir, type AgentBuildMode } from '../../lib/env';
import { passLinesFolded } from './foldedLines';
import { isDarwinTarget } from './platform';

/**
 * A path as the filesystem canonically names it, so ONE checkout reached by two
 * names (a symlinked home, `/tmp` -> `/private/tmp` on macOS) stays one checkout
 * and does not read as somebody else's worktree. Falls back to the resolved path
 * when the target cannot be resolved, which is the pre-canonical behavior.
 */
function canonical(dir: string): string {
  try {
    return fs.realpathSync(dir);
  } catch {
    return dir;
  }
}

/**
 * This checkout, canonical ONCE at module load. One const rather than a call at
 * each site: the stamp writer and the freshness check must record and compare
 * the same string by construction, not because two call sites happen to agree.
 */
const repoRoot = canonical(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..'));

/**
 * Short HEAD of the tree at `root`, or 'unknown'. Always `git -C <root>`, never
 * an inherited cwd: the harness is imported by an MCP server whose cwd is
 * whatever launched it, and a HEAD read from some other tree is worse than no
 * HEAD at all.
 */
export function gitHeadFor(root: string): string {
  try {
    return execFileSync('git', ['-C', root, 'rev-parse', '--short', 'HEAD'], {
      encoding: 'utf8',
      // stderr discarded, not inherited: a HEAD we could not read is already
      // handled below, and a bare `fatal: not a git repository` printed into a
      // session log reads like the harness itself broke.
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
  } catch {
    return 'unknown';
  }
}

/**
 * macOS has no external WebDriver daemon; the app itself must embed
 * tauri-plugin-webdriver-automation (cargo feature `e2e`) for tauri-wd to
 * attach. On Linux/Windows the plugin is not needed (and not wanted: the
 * feature adds an HTTP server to the binary). Keyed on the shared
 * targetPlatform() so build and session decisions cannot diverge.
 */
export function needsE2eFeature(): boolean {
  return isDarwinTarget();
}

// Source inputs whose changes should invalidate the built binary.
//
// This is the input surface of the WHOLE binary pipeline - vite's frontend
// leg AND cargo's backend leg, everything `pnpm tauri build` reads to produce
// the binary `checkBuildFreshness` is judging - not the tracked source tree
// in general, and it is a HEURISTIC, not a derivation from vite's or cargo's
// own dependency graphs. Issue #554: a green run over the wrong bundle is
// worse than a red one over the right question, so the bias here is to
// over-list rather than under-list. Missing entries (mtimeOf/newestMtimeInDir
// return 0 for files/dirs that do not exist, e.g. .env.local on a machine
// that has none) cost nothing; a missing entry that DOES exist and DOES feed
// the binary is the failure mode this list exists to close.
//
// `src-tauri/vendor` is included because it is not merely vendored
// documentation: `crates/parsers/Cargo.toml` depends on `compass_data` via a
// PATH dependency (`{ path = "../../vendor/compass_data" }`), so an edit
// there recompiles into the same binary despite the crate being excluded from
// the cargo workspace for lint-gate purposes only.
//
// The `.env*` set is vite's full mode-aware load order for `pnpm build`
// (mode=production, since `vite build` defaults to it): `.env`, `.env.local`,
// `.env.[mode]`, `.env.[mode].local` - all four, not the first three.
//
// The `.cargo/config.toml` (and legacy extensionless `.cargo/config`) entries
// are included at both the repo root and `src-tauri/`, because cargo reads
// this hierarchy from the build cwd upward and it shapes the binary (target
// dir, rustflags, profile overrides). These are commonly LOCAL-ONLY and
// uncommitted, which is exactly why the freshness scan - not git status -
// must be the thing that notices them.
//
// Exported (rather than kept module-private) so a test can table-drive over
// the list itself - dropping any single entry then goes red - instead of only
// pinning the handful an author remembered to write a case for.
export const SOURCE_DIRS = [
  'src',
  'public',
  path.join('src-tauri', 'src'),
  path.join('src-tauri', 'crates'),
  path.join('src-tauri', 'capabilities'),
  path.join('src-tauri', 'vendor'),
];
export const SOURCE_FILES = [
  path.join('src-tauri', 'tauri.conf.json'),
  path.join('src-tauri', 'Cargo.toml'),
  path.join('src-tauri', 'Cargo.lock'),
  path.join('src-tauri', 'build.rs'),
  'package.json',
  'pnpm-lock.yaml',
  'index.html',
  'vite.config.ts',
  'tsconfig.json',
  'postcss.config.js',
  'tailwind.config.js',
  '.env',
  '.env.local',
  '.env.production',
  '.env.production.local',
  path.join('.cargo', 'config.toml'),
  path.join('.cargo', 'config'),
  path.join('src-tauri', '.cargo', 'config.toml'),
  path.join('src-tauri', '.cargo', 'config'),
];

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
    if (entry.isDirectory()) {
      newest = Math.max(newest, newestMtimeInDir(full));
    } else {
      try {
        newest = Math.max(newest, fs.statSync(full).mtimeMs);
      } catch {
        // Skip unreadable files.
      }
    }
  }
  return newest;
}

function mtimeOf(file: string): number {
  try {
    return fs.statSync(file).mtimeMs;
  } catch {
    return 0;
  }
}

/**
 * Newest mtime (ms) among every file `checkBuildFreshness` treats as a build
 * input, under `root` (defaults to this checkout). Exported so the input list
 * (SOURCE_DIRS/SOURCE_FILES) is testable against a temp tree without touching
 * this checkout's real files - `root` is the seam.
 */
export function newestSourceMtime(root: string = repoRoot): number {
  let newest = 0;
  for (const dir of SOURCE_DIRS) {
    newest = Math.max(newest, newestMtimeInDir(path.join(root, dir)));
  }
  for (const file of SOURCE_FILES) {
    newest = Math.max(newest, mtimeOf(path.join(root, file)));
  }
  return newest;
}

/**
 * The frontend bundle's identity: md5 of `dist/index.html`, which is exactly
 * the `.e2e-build-marker` convention (from the retired scripts/wdio.sh) on its own
 * freshness path. `''` when there is no readable bundle to hash.
 *
 * md5 is a CHANGE DETECTOR here, not a security primitive - the file is a local
 * build output and the only thing being asked is "is this the one the build
 * embedded". Same digest as the shell marker so the two tiers agree.
 *
 * index.html alone, not the tree and not the binary: vite hashes every emitted
 * chunk into its filename and index.html references them by name, so a changed
 * chunk changes this file. Hashing more would cost a full read of the bundle (or
 * of a 200 MB debug binary) per start for signal this already carries.
 */
export function hashDistIndex(root: string = repoRoot): string {
  try {
    return createHash('md5').update(fs.readFileSync(path.join(root, 'dist', 'index.html'))).digest('hex');
  } catch {
    return '';
  }
}

/**
 * Stamp written by `buildApp` beside the binary it produced. Lives in the cargo
 * target dir (a build output, never the source tree, and outside the repo under
 * the default `~/.cargo_build`), so it is not something git can pick up.
 *
 * That location is also SHARED: several git worktrees of this repo point at one
 * `CARGO_TARGET_DIR`, so the binary a stamp vouches for may have been built from
 * a different tree than the one asking. mtimes cannot see that either - worktree
 * A's sources are typically older than a binary B just built, so the check
 * passed and A drove B's app (issues #488, #554). Since `v: 3` the stamp
 * therefore records WHICH tree built it and WHICH bundle that build embedded,
 * and a mismatch in either is stale.
 */
export interface E2EBuildStamp {
  /** Stamp schema version; an unknown version is treated as no stamp. */
  v: 3;
  /** Cargo profile this binary was built with. */
  mode: AgentBuildMode;
  /**
   * Every cargo feature the build passed, comma-joined and sorted ('' = none).
   *
   * Overlaps `e2eFeature` on purpose and does not replace it: that one answers
   * "does this binary carry the WebDriver plugin this PLATFORM needs", which
   * is a darwin question, while this one answers "was it built with what this
   * PROFILE asks for". A mac debug build and a linux release build both carry
   * `e2e`, for unrelated reasons, and a stamp has to say which.
   */
  features: string;
  /** Was the build run with VITE_E2E set (i.e. does dist carry the hooks)? */
  viteE2E: boolean;
  /**
   * Was the binary compiled with the `e2e` cargo feature (the in-app WebDriver
   * plugin tauri-wd needs on macOS)? mtimes cannot see this either: a
   * plugin-less mac binary would otherwise pass freshness and then hang at
   * session creation with no "[webdriver] listening" line ever printed.
   */
  e2eFeature: boolean;
  /** Vite mode the frontend leg was built in ('production' for `pnpm build`). */
  viteMode: string;
  /** mtime (ms) of the binary right after the build that wrote this stamp. */
  binaryMtimeMs: number;
  /**
   * md5 of the `dist/index.html` this build embedded (`''` if it was unreadable).
   *
   * The CONTENT backstop under the mtime heuristic, and the guard the retired wdio.sh
   * has had all along: a frontend rebuilt after the binary (a stray `pnpm build`,
   * a `dev.sh` run, a vitest that emitted a bundle) leaves the binary serving the
   * assets it was linked with while `dist/` holds different ones, which is the
   * "Could not connect to localhost" attach failure. An mtime cannot see it
   * whenever the clock says the binary is still the newer file.
   */
  distIndexHash: string;
  /**
   * Absolute, canonical path of the CHECKOUT this build was run from.
   *
   * One cargo target dir serves every worktree on the machine (issues #488,
   * #554), so without this a stamp written by worktree A vouches, field for
   * field, for a start in worktree B - same profile, same features, same
   * VITE_E2E, and a binary mtime that matches because it is literally the same
   * file. B then fuzzes A's code and reports the findings against B's diff.
   * Load-bearing, so a stamp without it is not a stamp.
   */
  repoRoot: string;
  /** Short HEAD of `repoRoot` at build time. Informational (shown in reasons). */
  gitHead: string;
  builtAtMs: number;
  builtAtIso: string;
  /**
   * `Date.now()` captured right BEFORE `buildApp` spawned `pnpm tauri build`,
   * i.e. before vite even started. Optional, and read leniently like
   * `gitHead` - a stamp without it (any stamp written before this field
   * existed) must still validate, just with the older, coarser comparison.
   *
   * Load-bearing when present: `pnpm tauri build`'s `beforeBuildCommand` runs
   * vite FIRST, emitting dist/ in seconds, and only then does cargo spend
   * minutes compiling and linking. A source edit landing in that window is
   * OLDER than the binary's final mtime - `newestSource > binMtime` therefore
   * misses it, distIndexHash still matches the dist/ that predates the edit,
   * and the whole stamp still checks out (issue #554). Comparing against the
   * moment the build STARTED, instead of the moment it finished, closes that
   * window: nothing edited after the build began can look fresh.
   */
  buildStartedAtMs?: number;
}

/**
 * Absolute path of the E2E build stamp for `mode`.
 *
 * One stamp per profile, living beside the binary it vouches for, so switching
 * between debug and release neither thrashes rebuilds nor lets a stale binary
 * from the other profile look fresh.
 */
export function buildStampPathFor(mode: AgentBuildMode): string {
  return path.join(cargoTargetDir, mode, '.stalagmight-e2e-build-stamp.json');
}

/** Stamp path for the DEBUG build (the default profile). */
export const buildStampPath = buildStampPathFor('debug');

/**
 * Cargo features each profile is built with.
 *
 * Release adds `e2e`, which is the repo's own name for this build: it keeps the
 * BODY of `debug_trigger_panic` compiled in (`#[cfg(any(debug_assertions,
 * feature = "e2e"))]` is on the body blocks, not on the command - the command
 * itself stays registered in every build so the invoke-handler and
 * telemetry-registry contracts are build-invariant, see
 * src-tauri/src/commands/debug.rs), so the crash-oracle self-test still works
 * without `debug_assertions`. Debug gets that for free and so passes nothing,
 * which keeps the DEFAULT build byte-for-byte what it has always been.
 *
 * WHAT ELSE THE FEATURE BRINGS, and it is not nothing: `e2e` also enables
 * `tauri-plugin-webdriver-automation` (`src-tauri/Cargo.toml`), which
 * `src-tauri/src/lib.rs` registers under `#[cfg(feature = "e2e")]`. So a
 * release session drives an app with one more plugin than a debug session does,
 * and "release differs only by compiled-out debug_asserts" is not the whole
 * truth - a finding that reproduces in one mode and not the other has a second
 * candidate explanation. The WDIO tier passes `--features e2e` for its DEBUG
 * build too (the retired scripts/wdio.sh did); this map deliberately does not, to leave the
 * agent harness's default build unchanged, and the asymmetry is recorded here
 * rather than discovered later.
 */
export const FEATURES_FOR_MODE: Record<AgentBuildMode, string[]> = {
  debug: [],
  release: ['e2e'],
};

/**
 * Every cargo feature a build of `mode` passes, on THIS platform.
 *
 * Two independent reasons to pass `--features e2e` - the PROFILE asks for it
 * (`FEATURES_FOR_MODE`, so release keeps `debug_trigger_panic`'s body) and the
 * PLATFORM asks for it (`needsE2eFeature`, so darwin gets the in-app WebDriver
 * plugin) - unioned into one answer.
 *
 * `buildApp` and `checkBuildFreshness` MUST derive from this same function.
 * They did not: the build unioned both reasons and stamped the result, while
 * the check asked the platform alone, so on Linux a release stamp said
 * `e2eFeature: true` and the check wanted `false`. `fresh` was then false the
 * instant the build finished - every `--release` start rebuilt from scratch,
 * and a fuzz-matrix run put K concurrent release builds on one target dir.
 */
export function featuresFor(mode: AgentBuildMode): string[] {
  return [...new Set([...FEATURES_FOR_MODE[mode], ...(needsE2eFeature() ? ['e2e'] : [])])].sort();
}

function featureKey(mode: AgentBuildMode): string {
  return featuresFor(mode).join(',');
}

/** Read + validate the stamp. Returns null if missing, unreadable, or bogus. */
export function readBuildStamp(file: string = buildStampPath): E2EBuildStamp | null {
  let raw: string;
  try {
    raw = fs.readFileSync(file, 'utf-8');
  } catch {
    return null;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== 'object' || parsed === null) return null;
  const candidate = parsed as Partial<E2EBuildStamp>;
  // A v1 stamp predates BOTH the e2eFeature field and the release mode, so it
  // can say neither whether the binary carries the WebDriver plugin nor which
  // profile it is. A v2 stamp predates the bundle hash and the checkout path, so
  // it cannot answer "is this dist the one you embedded" or "was it even this
  // worktree that built you". Both are treated as no stamp - one rebuild per
  // machine, once - rather than read leniently: a field a file does not carry is
  // a check that silently does not run, which is the shape of #488 itself.
  //
  // `gitHead` and `buildStartedAtMs` are the exceptions, and deliberately so:
  // neither decides validity, and a stamp written by a sibling branch (or by
  // this branch before #554) that does not record them must still be READABLE
  // here, or the two halves of a shared target dir would rebuild over each
  // other on every start. `buildStartedAtMs` absent just means the caller falls
  // back to the coarser binMtime comparison, same as before this field existed.
  if (candidate.v !== 3) return null;
  if (candidate.mode !== 'debug' && candidate.mode !== 'release') return null;
  if (typeof candidate.features !== 'string') return null;
  if (typeof candidate.viteE2E !== 'boolean') return null;
  if (typeof candidate.e2eFeature !== 'boolean') return null;
  if (typeof candidate.binaryMtimeMs !== 'number') return null;
  if (typeof candidate.distIndexHash !== 'string') return null;
  if (typeof candidate.repoRoot !== 'string' || candidate.repoRoot === '') return null;
  return {
    v: 3,
    mode: candidate.mode,
    features: candidate.features,
    viteE2E: candidate.viteE2E,
    e2eFeature: candidate.e2eFeature,
    viteMode: typeof candidate.viteMode === 'string' ? candidate.viteMode : 'unknown',
    binaryMtimeMs: candidate.binaryMtimeMs,
    distIndexHash: candidate.distIndexHash,
    repoRoot: candidate.repoRoot,
    gitHead: typeof candidate.gitHead === 'string' ? candidate.gitHead : 'unknown',
    builtAtMs: typeof candidate.builtAtMs === 'number' ? candidate.builtAtMs : 0,
    builtAtIso: typeof candidate.builtAtIso === 'string' ? candidate.builtAtIso : '',
    // Lenient, same as gitHead: a non-number is dropped rather than failing the
    // whole stamp, so a hand-edited or foreign-written field cannot poison an
    // otherwise-valid stamp into rejecting every start.
    ...(typeof candidate.buildStartedAtMs === 'number' ? { buildStartedAtMs: candidate.buildStartedAtMs } : {}),
  };
}

/**
 * Pure staleness decision for the stamp half of the check. Returns the reasons
 * the current binary cannot be trusted to carry the requested VITE_E2E state;
 * an empty array means the stamp vouches for the binary.
 */
export function stampStaleReasons(
  stamp: E2EBuildStamp | null,
  want: {
    viteE2E: boolean;
    e2eFeature: boolean;
    binaryMtimeMs: number;
    /** md5 of the bundle on disk NOW; `''` when there is none to hash. */
    distIndexHash: string;
    /** Canonical path of the checkout this start runs from. */
    repoRoot: string;
    mode?: AgentBuildMode | undefined;
    features?: string | undefined;
  },
): string[] {
  if (!stamp) {
    return ['no E2E build stamp: the binary was not built by this harness'];
  }
  const reasons: string[] = [];
  if (want.mode !== undefined && stamp.mode !== want.mode) {
    reasons.push(`build stamp is for the ${stamp.mode} profile but this start needs ${want.mode}`);
  }
  if (want.features !== undefined && stamp.features !== want.features) {
    reasons.push(
      `build stamp was built with features "${stamp.features || '(none)'}" but this start needs ` +
        `"${want.features || '(none)'}"`,
    );
  }
  if (stamp.viteE2E !== want.viteE2E) {
    reasons.push(
      `build stamp says VITE_E2E=${stamp.viteE2E ? '1' : '0'} but this start needs ` +
        `VITE_E2E=${want.viteE2E ? '1' : '0'}`,
    );
  }
  if (stamp.e2eFeature !== want.e2eFeature) {
    reasons.push(
      `build stamp says the e2e cargo feature was ${stamp.e2eFeature ? 'on' : 'off'} ` +
        `but this start needs it ${want.e2eFeature ? 'on' : 'off'} ` +
        `(the profile and the platform both ask - see featuresFor)`,
    );
  }
  if (stamp.binaryMtimeMs !== want.binaryMtimeMs) {
    reasons.push('the binary was rebuilt outside the harness since the E2E build stamp');
  }
  if (stamp.repoRoot !== want.repoRoot) {
    reasons.push(
      `build stamp was produced by the checkout at ${stamp.repoRoot} (@ ${stamp.gitHead}) but this ` +
        `start runs from ${want.repoRoot} - one cargo target dir serves every worktree, so that ` +
        `binary is not this tree's`,
    );
  }
  if (want.distIndexHash === '') {
    // Not "assume it is fine": the binary embeds a bundle, and with nothing on
    // disk to hash there is no way to tell WHICH one. The retired wdio.sh refused
    // the same way ("Frontend dist not found"), and the cost is one rebuild for
    // whoever deleted `dist/`.
    reasons.push('dist/index.html is missing, so the bundle this binary embeds cannot be verified');
  } else if (stamp.distIndexHash !== want.distIndexHash) {
    reasons.push(
      `the frontend bundle moved since the build: dist/index.html hashes to ${want.distIndexHash} ` +
        `but the stamp records distIndexHash ${stamp.distIndexHash || '(none)'}`,
    );
  }
  return reasons;
}

/**
 * Slack absorbed when checking `buildStartedAtMs` against `binaryMtimeMs` in
 * the same stamp, to cover legitimate filesystem mtime granularity rather
 * than genuine clock skew: sub-millisecond on ext4/APFS, up to ~1s on HFS+,
 * up to ~2s on FAT - 2000ms is the FAT worst case.
 */
const BUILD_START_SLACK_MS = 2000;

/**
 * Pure staleness decision for the file-TIMING half of the check: is
 * `newestSource` (the newest mtime among tracked build inputs) newer than the
 * moment the binary can be trusted to reflect?
 *
 * Issue #554: `pnpm tauri build`'s beforeBuildCommand runs vite FIRST, so
 * dist/ lands within seconds while cargo then spends minutes compiling and
 * linking. A source edit landing in that cargo window is OLDER than the
 * binary's eventual mtime, so comparing against `binMtime` (the pre-#554
 * behavior) misses it - the bundle on disk predates the edit but the mtime
 * check still says fresh. Comparing against the moment the build STARTED
 * closes that window, but only when a stamp is in hand to say when that was;
 * a stampless binary is already stale via `stampStaleReasons` above, so its
 * branch here keeps the original `binMtime` comparison to avoid restating
 * "no stamp" as a second, confusing source-timing complaint.
 *
 * A stamp older than #554 (no `buildStartedAtMs`) falls back to its own
 * `binaryMtimeMs`, i.e. exactly the pre-fix comparison - lenient, not a forced
 * rebuild, same as every other optional stamp field.
 *
 * `buildStartedAtMs` is trusted only when it is a plausible start time for
 * the binary the SAME stamp records, i.e. `0 < buildStartedAtMs <=
 * binaryMtimeMs + BUILD_START_SLACK_MS` - a real build starts before (or
 * around) the instant it finishes linking. Two clocks feed this field (the
 * harness process clock for `buildStartedAtMs`, the filesystem for
 * `binaryMtimeMs`), and they are not guaranteed to agree even for a CORRECT
 * build: filesystem mtime granularity can legitimately put `binaryMtimeMs`
 * BELOW `buildStartedAtMs` - sub-millisecond truncation on ext4/APFS (seen in
 * CI: a binary stamped 0.08ms below its own `Date.now()`), up to ~1s on
 * HFS+, up to ~2s on FAT - and an exact `<=` guard would misclassify every
 * one of those as clock-skewed and silently lose the mid-build race
 * protection on exactly the filesystems coarse enough to need it.
 * `BUILD_START_SLACK_MS` absorbs that legitimate case; only skew LARGER than
 * the coarsest granularity in wide use is treated as untrustworthy: an NTP
 * step backwards between the two reads, a hand-edited stamp, or a filesystem
 * whose mtimes run ahead of the process clock (VM/Docker shared mounts, some
 * CI volumes). Trusting an actually-skewed value would be worse in both
 * directions than the pre-#554 behavior it replaces: a future-dated value
 * makes every later edit read fresh forever (case (a)), and a filesystem
 * lagging the process clock makes every source read newer than
 * `binaryMtimeMs` (case (b)) - and because each rebuild re-stamps with the
 * same lagging clock, that direction never converges, silently defeating
 * `fuzz-matrix`'s warm-up-alone skip too. Falling back to `binaryMtimeMs`
 * whenever the invariant fails keeps both failure modes bounded to "no worse
 * than before this fix".
 *
 * Within the accepted range the threshold is `Math.min(buildStartedAtMs,
 * binaryMtimeMs)`, not `buildStartedAtMs` outright - stale-biased on both
 * sides. A normal build uses `buildStartedAtMs` (the smaller value, closing
 * the mid-build race). A build whose mtime floored below its start time uses
 * `binaryMtimeMs` instead, which can only make MORE sources read as stale,
 * never fewer - the bias this whole check exists for.
 */
export function sourceStaleReason(newestSource: number, binMtime: number, stamp: E2EBuildStamp | null): string | null {
  if (stamp) {
    const startedAt = stamp.buildStartedAtMs;
    const trustworthyStart =
      startedAt !== undefined && startedAt > 0 && startedAt <= stamp.binaryMtimeMs + BUILD_START_SLACK_MS;
    const threshold = trustworthyStart ? Math.min(startedAt, stamp.binaryMtimeMs) : stamp.binaryMtimeMs;
    if (newestSource > threshold) {
      return trustworthyStart
        ? 'a source file changed after the build started'
        : 'a source file is newer than the binary';
    }
    return null;
  }
  return newestSource > binMtime ? 'a source file is newer than the binary' : null;
}

/**
 * Every reason `checkBuildFreshness` would refuse the binary, over inputs it
 * already computed - the stamp half (`stampStaleReasons`) plus the file-timing
 * half (`sourceStaleReason`), composed exactly as `checkBuildFreshness` uses
 * them. Exported (and unit-tested directly) so the WIRING between the two
 * halves - both landing in the same reasons array - is itself covered: a
 * mutation that drops the `sourceStaleReason` call, or that drops it silently
 * on the fresh-stamp path, fails a test on this function without needing a
 * real binary or a real build.
 */
export function freshnessReasons(input: {
  binMtime: number;
  stamp: E2EBuildStamp | null;
  newestSource: number;
  viteE2E: boolean;
  e2eFeature: boolean;
  distIndexHash: string;
  repoRoot: string;
  mode?: AgentBuildMode;
  features?: string;
}): string[] {
  const reasons = stampStaleReasons(input.stamp, {
    viteE2E: input.viteE2E,
    e2eFeature: input.e2eFeature,
    binaryMtimeMs: input.binMtime,
    distIndexHash: input.distIndexHash,
    repoRoot: input.repoRoot,
    mode: input.mode,
    features: input.features,
  });
  const sourceReason = sourceStaleReason(input.newestSource, input.binMtime, input.stamp);
  if (sourceReason) reasons.push(sourceReason);
  return reasons;
}

export interface Freshness {
  fresh: boolean;
  reasons: string[];
  binaryPath: string;
  /**
   * Which tree built the binary, per its stamp; null when no stamp vouches for
   * it. Surfaced so a caller can report builder identity without re-reading
   * (and re-validating) the stamp itself.
   */
  builtFrom: BuiltFrom;
}

/** Builder identity, or null when no valid stamp vouches for this binary. */
export type BuiltFrom = { repoRoot: string; gitHead: string } | null;

/**
 * Who built the binary for `mode`, per the stamp beside it.
 *
 * Null when NO STAMP VOUCHES FOR THE BINARY THAT IS THERE - which covers a
 * missing stamp, a v1/v2/corrupt one, a stamp our own build failed to write,
 * and the case this guard exists for: a stamp whose `binaryMtimeMs` no longer
 * matches the file. `./scripts/dev.sh` or a plain `pnpm tauri build` relinks the
 * binary in place and leaves the old stamp sitting next to it; the stamper did
 * not build THAT binary, so answering with its identity would be a confident
 * lie. Callers are told to trust this on the `build: "skip"` path, where it is
 * the only thing standing between them and a foreign binary, so it says nothing
 * rather than something wrong.
 *
 * `stampFile`/`binaryFile` default to the real paths and exist so the guard is
 * testable against a temp pair, the same convention `readBuildStamp` follows.
 */
export function builtFromFor(
  mode: AgentBuildMode,
  stampFile: string = buildStampPathFor(mode),
  binaryFile: string = binaryPathFor(mode),
): BuiltFrom {
  const stamp = readBuildStamp(stampFile);
  if (!stamp) return null;
  if (stamp.binaryMtimeMs !== mtimeOf(binaryFile)) return null;
  return { repoRoot: stamp.repoRoot, gitHead: stamp.gitHead };
}

export interface FreshnessOptions {
  /** Does this start need the VITE_E2E frontend hooks? Defaults to true. */
  requireE2E?: boolean;
  /** Profile to check. Defaults to `agentBuildMode()`. */
  mode?: AgentBuildMode;
}

/**
 * Cheap heuristic: is the built binary newer than all tracked sources, and does
 * the E2E build stamp vouch for it carrying the requested VITE_E2E state?
 */
export function checkBuildFreshness(opts: FreshnessOptions = {}): Freshness {
  const requireE2E = opts.requireE2E ?? true;
  const mode = opts.mode ?? agentBuildMode();
  const binaryPath = binaryPathFor(mode);
  const reasons: string[] = [];
  const binMtime = mtimeOf(binaryPath);
  if (binMtime === 0) {
    return {
      fresh: false,
      reasons: [`${mode} binary missing at ${binaryPath}`],
      binaryPath,
      builtFrom: null,
    };
  }

  // Read ONCE: the staleness decision and the reported builder identity must be
  // the same stamp, not two reads that a concurrent build could split. Callers
  // are expected to take `builtFrom` from here on the fresh path rather than ask
  // again (see session.ts) - that is what makes this single read hold.
  const stamp = readBuildStamp(buildStampPathFor(mode));
  // Same rule as `builtFromFor`: a stamp only speaks for the binary whose mtime
  // it recorded. On the fresh path the two are equal by construction (a
  // mismatch is one of the reasons below); saying null otherwise keeps one
  // meaning for `builtFrom` across both entry points.
  const builtFrom: BuiltFrom =
    stamp && stamp.binaryMtimeMs === binMtime
      ? { repoRoot: stamp.repoRoot, gitHead: stamp.gitHead }
      : null;
  reasons.push(
    ...freshnessReasons({
      binMtime,
      stamp,
      newestSource: newestSourceMtime(repoRoot),
      viteE2E: requireE2E,
      e2eFeature: featuresFor(mode).includes('e2e'),
      distIndexHash: hashDistIndex(),
      // Production always knows which tree it is, and a shared CARGO_TARGET_DIR
      // means another worktree's binary must not pass as fresh.
      repoRoot,
      mode,
      features: featureKey(mode),
    }),
  );
  return { fresh: reasons.length === 0, reasons, binaryPath, builtFrom };
}

/**
 * Build the debug binary with the E2E frontend hooks. Blocking (this is a slow
 * one-time cost). Throws on build failure.
 */
// The feature build regenerates these TRACKED files with the WebDriver
// plugin's ACL entries; the repo keeps the plain-build shape.
const SCHEMA_PATHSPEC = 'src-tauri/gen/schemas';
// Every ACL entry the plugin adds carries its crate name, so a diff whose added
// lines all mention it is harness churn and nothing else.
const PLUGIN_ACL_MARKER = 'webdriver-automation';

/** True when the tracked generated schemas have no uncommitted changes. */
function schemasClean(): boolean {
  try {
    execSync(`git diff --quiet -- ${SCHEMA_PATHSPEC}`, { cwd: repoRoot, stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

function schemaDiff(): string {
  try {
    // stderr discarded, as in `gitHeadFor`: a diff we could not read is '' below,
    // and git's own error can name the checkout.
    return execSync(`git diff -- ${SCHEMA_PATHSPEC}`, {
      cwd: repoRoot,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    });
  } catch {
    return '';
  }
}

/**
 * Is this schema diff nothing but the WebDriver plugin's ACL churn? Every added
 * line must mention the plugin; a single unrelated addition makes it somebody's
 * real work, which tooling must never discard.
 *
 * Pure (takes the diff text) so the classification is unit-testable without a
 * dirty worktree.
 */
export function isPluginAclChurn(diff: string): boolean {
  const lines = diff.split('\n');
  // A removal is never plugin churn: the feature build only ADDS ACL entries.
  // Any removed line therefore means a human edited these files, and restoring
  // would silently discard that edit - the exact data loss this guard exists to
  // prevent. (Missing this was a review finding: classifying on additions alone
  // called "your deletion + our churn" pure churn.)
  if (lines.some((line) => line.startsWith('-') && !line.startsWith('---'))) return false;
  const added = lines
    .filter((line) => line.startsWith('+') && !line.startsWith('+++'))
    .map((line) => line.slice(1).trim())
    .filter((line) => line.length > 0);
  return added.length > 0 && added.every((line) => line.includes(PLUGIN_ACL_MARKER));
}

/**
 * Runs the build `command` in a shell, its stdout and its stderr both passed to
 * this process's stderr a line at a time with the checkout folded to `<repo>`
 * and the home directory to `~` (`passLinesFolded`, #576 W47-NF3). Inherited,
 * cargo's `Compiling <crate> (<path>)` and Tauri's `Built application at:
 * <path>` printed raw beside a runner's shareable lines - `agent:scenario`,
 * `agent:journeys` and `agent:smoke` build in-process. Rejects as `execSync`
 * threw, `Command failed: <command>`, when the build exits non-zero or dies.
 */
function runBuildCommand(command: string, options: { cwd: string; env: NodeJS.ProcessEnv }): Promise<void> {
  return new Promise((resolve, reject) => {
    // stdout too goes to stderr, NEVER to this process's stdout: under the MCP
    // server, stdout is the JSON-RPC stream and any build chatter (pnpm/vite log
    // to stdout) would corrupt the protocol framing.
    const child = spawn(command, { ...options, shell: true, stdio: ['ignore', 'pipe', 'pipe'] });
    const write = (text: string): void => {
      process.stderr.write(text);
    };
    const passed = Promise.all([passLinesFolded(child.stdout, write), passLinesFolded(child.stderr, write)]);
    child.once('error', reject);
    child.once('close', (code: number | null) => {
      void passed.then(() => (code === 0 ? resolve() : reject(new Error(`Command failed: ${command}`))));
    });
  });
}

export async function buildApp(
  onLog?: (line: string) => void,
  mode: AgentBuildMode = agentBuildMode(),
): Promise<void> {
  // Two independent reasons to pass `--features e2e`, UNIONED rather than
  // either-or: the PROFILE asks for it (release keeps `debug_trigger_panic`'s
  // body compiled in - `FEATURES_FOR_MODE`), and the PLATFORM asks for it
  // (darwin needs the in-app WebDriver plugin - `needsE2eFeature`). A mac
  // release build needs it once, not twice.
  const features = featuresFor(mode);
  const withE2eFeature = features.includes('e2e');
  // `--debug` selects the dev profile; its absence selects release. The frontend
  // leg is identical either way - `beforeBuildCommand` runs `pnpm build`, and
  // VITE_E2E=1 is what makes it emit the `window.__E2E_TEST__` hooks (the gate in
  // src/main.tsx is DEV || MODE==='test' || VITE_E2E==='1', so it works in a
  // production-mode vite build too).
  const profileFlag = mode === 'debug' ? ' --debug' : '';
  // The cargo-PASSTHROUGH form, which is what the WDIO tier uses and what has
  // been exercised on both platforms.
  const featureFlag = features.length > 0 ? ` -- --features ${features.join(',')}` : '';
  const command = `pnpm tauri build${profileFlag} --no-bundle${featureFlag}`;
  // Restore only churn that is provably the plugin's. Timing alone is not
  // enough: an interrupted feature build (a killed smoke, ctrl-c) leaves churn
  // behind, and a "was it clean before?" rule would then refuse to clean up
  // forever after. Classifying the DIFF instead recovers from that while still
  // never discarding a real edit to these files.
  const preBuildDirtIsOurs = withE2eFeature && (schemasClean() || isPluginAclChurn(schemaDiff()));
  onLog?.(
    `Building Tauri ${mode} app (VITE_E2E=1, --no-bundle${withE2eFeature ? ', --features e2e' : ''})...`,
  );
  // Captured BEFORE the spawn, not after: `pnpm tauri build`'s
  // beforeBuildCommand runs vite first (dist/ lands in seconds) and only then
  // does cargo spend minutes compiling and linking, so a source edit made
  // during the cargo leg is older than the binary's eventual mtime. Stamping
  // the start, not the finish, is what makes `checkBuildFreshness` catch it.
  const buildStartedAtMs = Date.now();
  // Build output goes to stderr, folded - see runBuildCommand.
  await runBuildCommand(command, {
    cwd: repoRoot,
    env: {
      ...process.env,
      VITE_E2E: '1',
      CARGO_TARGET_DIR: cargoTargetDir,
    },
  });
  if (withE2eFeature && !schemasClean()) {
    if (preBuildDirtIsOurs && isPluginAclChurn(schemaDiff())) {
      try {
        execSync(`git checkout -- ${SCHEMA_PATHSPEC}`, { cwd: repoRoot, stdio: 'ignore' });
        onLog?.(`Restored ${SCHEMA_PATHSPEC} (the e2e-feature build regenerates them with plugin ACL entries).`);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        onLog?.(`Warning: could not restore ${SCHEMA_PATHSPEC} (${msg}); do NOT commit them in this shape.`);
      }
    } else {
      onLog?.(
        `Warning: ${SCHEMA_PATHSPEC} carries changes beyond the plugin's ACL entries, so the ` +
          `harness left it alone rather than discarding real work - it now ALSO carries ` +
          `e2e-feature churn. Do not commit the plugin entries.`,
      );
    }
  }
  writeBuildStamp(
    {
      viteE2E: true,
      e2eFeature: withE2eFeature,
      viteMode: 'production',
      mode,
      features: features.join(','),
      buildStartedAtMs,
    },
    onLog,
  );
  onLog?.(`Build complete (${mode}).`);
}

/**
 * Record what the build we just ran produced, so the next `build: "auto"` can
 * tell a hookless binary from an E2E one without attaching first. Best-effort:
 * a stamp we fail to write just means the next start rebuilds.
 *
 * `binaryFile`/`stampPath` default to the real paths and exist so this is
 * testable against a temp binary, the same convention `builtFromFor` follows -
 * in particular so a test can pin the real writer's own invariant, that the
 * `buildStartedAtMs` it records never exceeds the `binaryMtimeMs` it reads off
 * the same binary (see `sourceStaleReason`'s guard against a stamp violating
 * that).
 */
export function writeBuildStamp(
  info: {
    viteE2E: boolean;
    e2eFeature: boolean;
    viteMode: string;
    mode: AgentBuildMode;
    features: string;
    buildStartedAtMs: number;
  },
  onLog?: (line: string) => void,
  binaryFile: string = binaryPathFor(info.mode),
  stampPath: string = buildStampPathFor(info.mode),
): void {
  const now = Date.now();
  const stamp: E2EBuildStamp = {
    v: 3,
    mode: info.mode,
    features: info.features,
    viteE2E: info.viteE2E,
    e2eFeature: info.e2eFeature,
    viteMode: info.viteMode,
    binaryMtimeMs: mtimeOf(binaryFile),
    distIndexHash: hashDistIndex(),
    repoRoot,
    gitHead: gitHeadFor(repoRoot),
    builtAtMs: now,
    builtAtIso: new Date(now).toISOString(),
    buildStartedAtMs: info.buildStartedAtMs,
  };
  try {
    fs.mkdirSync(path.dirname(stampPath), { recursive: true });
    fs.writeFileSync(stampPath, `${JSON.stringify(stamp, null, 2)}\n`);
  } catch (err) {
    // The errno code, never the message: an fs message quotes the stamp's path,
    // and CARGO_TARGET_DIR can sit outside the checkout and the home directory,
    // where the caller's fold does not reach (#576 W51).
    const code = (err as NodeJS.ErrnoException).code ?? 'io';
    onLog?.(`Warning: could not write E2E build stamp (${code}); next start will rebuild.`);
  }
}
