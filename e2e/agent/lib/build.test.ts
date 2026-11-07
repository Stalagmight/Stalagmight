/**
 * Unit tests for the agent-harness build-freshness decision (issue #260): the
 * E2E build stamp must make a hookless binary look STALE *before* attach, and
 * an E2E binary the harness itself built must still look fresh (no
 * always-rebuild regression). Since #421 the stamp also records whether the
 * binary carries the e2e cargo feature (the in-app WebDriver plugin macOS
 * needs), and a v1 stamp reads as no stamp at all. Since #488 it also records
 * WHICH source tree built the binary, because several worktrees share one
 * CARGO_TARGET_DIR and mtimes cannot tell one builder from another.
 */
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  FEATURES_FOR_MODE,
  SOURCE_DIRS,
  SOURCE_FILES,
  buildStampPathFor,
  builtFromFor,
  featuresFor,
  freshnessReasons,
  gitHeadFor,
  hashDistIndex,
  isPluginAclChurn,
  needsE2eFeature,
  newestSourceMtime,
  readBuildStamp,
  sourceStaleReason,
  stampStaleReasons,
  writeBuildStamp,
  type E2EBuildStamp,
} from './build';

/** The harness tree this test file lives in, in the form the stamp records. */
const HARNESS_ROOT_REAL = fs.realpathSync(
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..'),
);

const E2E_STAMP: E2EBuildStamp = {
  v: 3,
  mode: 'debug',
  features: '',
  viteE2E: true,
  e2eFeature: false,
  viteMode: 'production',
  binaryMtimeMs: 1_700_000_000_000,
  distIndexHash: 'd41d8cd98f00b204e9800998ecf8427e',
  repoRoot: '/repo/a',
  gitHead: 'abc1234',
  builtAtMs: 1_700_000_000_500,
  builtAtIso: '2023-11-14T22:13:20.500Z',
};

// `distIndexHash` and `repoRoot` are MANDATORY in the want: a caller that cannot
// say which bundle is on disk or which checkout it is has not asked a question
// the stamp can answer, and defaulting either would silently skip a check.
const wantMatching = {
  viteE2E: true,
  e2eFeature: false,
  binaryMtimeMs: E2E_STAMP.binaryMtimeMs,
  distIndexHash: E2E_STAMP.distIndexHash,
  repoRoot: E2E_STAMP.repoRoot,
};

describe('stampStaleReasons', () => {
  it('treats a missing stamp as stale (binary not built by the harness)', () => {
    const reasons = stampStaleReasons(null, {
      viteE2E: true,
      e2eFeature: false,
      binaryMtimeMs: 1,
      distIndexHash: 'abc',
      repoRoot: '/repo/a',
    });
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toContain('no E2E build stamp');
  });

  it('treats a non-E2E stamp as stale when the start needs the hooks', () => {
    const stamp = { ...E2E_STAMP, viteE2E: false };
    const reasons = stampStaleReasons(stamp, wantMatching);
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toContain('VITE_E2E=0');
  });

  it('treats an E2E stamp as stale when the start does NOT want the hooks', () => {
    const reasons = stampStaleReasons(E2E_STAMP, { ...wantMatching, viteE2E: false });
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toContain('VITE_E2E=1');
  });

  it('treats a plugin-less binary as stale when the platform needs the e2e feature', () => {
    const reasons = stampStaleReasons(E2E_STAMP, { ...wantMatching, e2eFeature: true });
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toContain('e2e cargo feature');
    expect(reasons[0]).toContain('off');
  });

  it('treats a feature build as stale when the platform wants it off', () => {
    const stamp = { ...E2E_STAMP, e2eFeature: true };
    const reasons = stampStaleReasons(stamp, wantMatching);
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toContain('e2e cargo feature');
    expect(reasons[0]).toContain('on');
  });

  it('treats a binary rebuilt behind our back as stale', () => {
    const reasons = stampStaleReasons(E2E_STAMP, {
      ...wantMatching,
      binaryMtimeMs: E2E_STAMP.binaryMtimeMs + 1000,
    });
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toContain('rebuilt outside the harness');
  });

  it('accepts a matching E2E stamp (no rebuild - the whole point)', () => {
    expect(stampStaleReasons(E2E_STAMP, wantMatching)).toEqual([]);
  });

  it('accepts a matching feature-build stamp', () => {
    const stamp = { ...E2E_STAMP, e2eFeature: true };
    expect(stampStaleReasons(stamp, { ...wantMatching, e2eFeature: true })).toEqual([]);
  });

  // Issue #488: worktrees share one CARGO_TARGET_DIR, so the binary a stamp sits
  // beside may have been built somewhere else entirely. mtimes say nothing about
  // it - the OTHER tree's sources are the ones that were touched - so without
  // this comparison worktree A happily drove the app worktree B built.
  it('treats a binary built from another source tree as stale', () => {
    const reasons = stampStaleReasons(E2E_STAMP, { ...wantMatching, repoRoot: '/repo/b' });
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toContain('/repo/a');
    expect(reasons[0]).toContain('/repo/b');
    // The builder's HEAD rides along in the reason: "which tree" alone does not
    // tell you what code you were about to drive.
    expect(reasons[0]).toContain(E2E_STAMP.gitHead);
    expect(reasons[0]).toContain('one cargo target dir serves every worktree');
  });

  it('accepts a stamp written by the tree that is asking', () => {
    expect(stampStaleReasons(E2E_STAMP, { ...wantMatching, repoRoot: E2E_STAMP.repoRoot })).toEqual([]);
  });

  // The bundle half of the same hazard: the binary embeds a dist, and a dist
  // rebuilt behind it is invisible to every other field in the stamp.
  it('treats a bundle that moved since the build as stale', () => {
    const reasons = stampStaleReasons(E2E_STAMP, { ...wantMatching, distIndexHash: 'ffff0000' });
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toContain('the frontend bundle moved since the build');
    expect(reasons[0]).toContain('ffff0000');
    expect(reasons[0]).toContain(E2E_STAMP.distIndexHash);
  });

  it('treats a missing dist as stale rather than assuming the bundle is fine', () => {
    const reasons = stampStaleReasons(E2E_STAMP, { ...wantMatching, distIndexHash: '' });
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toBe(
      'dist/index.html is missing, so the bundle this binary embeds cannot be verified',
    );
  });

  it('reports a missing dist once, not also as a hash mismatch', () => {
    // The stamp records a REAL hash and the want is '': the two dist cases are
    // exclusive (if/else if), so this must produce the missing-dist reason
    // alone. Degrade that else-if to a plain if and this case reports twice,
    // which a stamp/want pair that both said '' could never have caught.
    const reasons = stampStaleReasons(
      { ...E2E_STAMP, distIndexHash: 'abc' },
      { ...wantMatching, distIndexHash: '' },
    );
    expect(reasons).toHaveLength(1);
    expect(reasons[0]).toContain('dist/index.html is missing');
  });
});

// Issue #554: `pnpm tauri build`'s beforeBuildCommand runs vite first (dist/
// lands in seconds) and only THEN does cargo compile/link for minutes, so a
// source edit landing in the cargo window is older than the binary's final
// mtime. Comparing against `buildStartedAtMs` instead of `binMtime` is the
// fix; these pin the three-way branch (raced, fallback, no-stamp) directly on
// the pure decision function `checkBuildFreshness` delegates to.
describe('sourceStaleReason', () => {
  const BUILD_STARTED = 1_700_000_000_000;
  const BUILD_FINISHED = 1_700_000_060_000; // +60s: the cargo compile/link leg.
  const stampWithStart: E2EBuildStamp = {
    ...E2E_STAMP,
    binaryMtimeMs: BUILD_FINISHED,
    buildStartedAtMs: BUILD_STARTED,
  };
  // An explicit omit, not `{ ...E2E_STAMP, binaryMtimeMs: X }`: if the shared
  // fixture ever grows a `buildStartedAtMs` of its own, a spread-based "old
  // stamp" would silently stop testing the fallback path these tests exist
  // for. Built field-by-field so TypeScript itself would flag a missing
  // required field, and `buildStartedAtMs` is provably absent by construction.
  function oldV3Stamp(binaryMtimeMs: number): E2EBuildStamp {
    return {
      v: 3,
      mode: E2E_STAMP.mode,
      features: E2E_STAMP.features,
      viteE2E: E2E_STAMP.viteE2E,
      e2eFeature: E2E_STAMP.e2eFeature,
      viteMode: E2E_STAMP.viteMode,
      binaryMtimeMs,
      distIndexHash: E2E_STAMP.distIndexHash,
      repoRoot: E2E_STAMP.repoRoot,
      gitHead: E2E_STAMP.gitHead,
      builtAtMs: E2E_STAMP.builtAtMs,
      builtAtIso: E2E_STAMP.builtAtIso,
      // buildStartedAtMs intentionally omitted.
    };
  }

  // RED-FIRST: injection is `newestSource` landing strictly between the two
  // timestamps - the mid-build edit. Against the OLD comparison
  // (`newestSource > binMtime`, i.e. `> BUILD_FINISHED`) this same input is
  // NOT stale: `BUILD_STARTED + 30_000 = 1_700_000_030_000`, which is well
  // under `BUILD_FINISHED = 1_700_000_060_000`. That is exactly the #554 bug -
  // the mid-build edit reads as fresh under the pre-fix comparison and only
  // the new buildStartedAtMs-based one catches it.
  it('flags a source edit that landed after the build started but before it finished', () => {
    const midBuildEdit = BUILD_STARTED + 30_000;
    expect(sourceStaleReason(midBuildEdit, BUILD_FINISHED, stampWithStart)).toBe(
      'a source file changed after the build started',
    );
  });

  it('does not flag a source edit that predates the build start', () => {
    expect(sourceStaleReason(BUILD_STARTED - 1000, BUILD_FINISHED, stampWithStart)).toBeNull();
  });

  it('flags a source edit after the binary finished too (still newer than everything)', () => {
    expect(sourceStaleReason(BUILD_FINISHED + 1000, BUILD_FINISHED, stampWithStart)).toBe(
      'a source file changed after the build started',
    );
  });

  // Fallback: injection is a v3 stamp predating #554 (no `buildStartedAtMs`).
  // Turns red if the fallback used `binMtime` (the live binary's CURRENT mtime,
  // which callers pass separately) instead of the stamp's OWN `binaryMtimeMs` -
  // the two are equal on the fresh path but a test that conflated them would
  // not catch a caller passing the wrong one.
  it('falls back to the stamp binaryMtimeMs when buildStartedAtMs is absent (old v3 stamp)', () => {
    const oldStamp = oldV3Stamp(BUILD_FINISHED);
    // A source older than the binary: must NOT be stale under the fallback.
    expect(sourceStaleReason(BUILD_FINISHED - 1000, BUILD_FINISHED, oldStamp)).toBeNull();
    // A source newer than the binary: must still be caught.
    expect(sourceStaleReason(BUILD_FINISHED + 1000, BUILD_FINISHED, oldStamp)).toBe(
      'a source file is newer than the binary',
    );
  });

  // Stampless path: injection is `stamp: null` with `newestSource > binMtime`.
  // Turns red if the null branch were dropped in favor of always reading
  // `stamp!.binaryMtimeMs`, which would throw rather than fall back.
  it('uses binMtime directly when there is no stamp at all', () => {
    expect(sourceStaleReason(BUILD_FINISHED + 1000, BUILD_FINISHED, null)).toBe(
      'a source file is newer than the binary',
    );
    expect(sourceStaleReason(BUILD_FINISHED - 1000, BUILD_FINISHED, null)).toBeNull();
  });

  // Pre-PR review finding: buildStartedAtMs and binaryMtimeMs are two
  // independent clocks (the harness process clock vs. the filesystem) and are
  // not guaranteed to agree. A `buildStartedAtMs` far LATER than the same
  // stamp's `binaryMtimeMs` cannot describe a real build (a build cannot start
  // long after it finished linking) - candidates are an NTP step backwards, a
  // hand-edited stamp, or a filesystem whose mtimes trail the process clock.
  // Second review round (CI failure on PR #574): the guard must NOT be exact
  // `<=` - filesystem mtime granularity can legitimately put `binaryMtimeMs`
  // fractionally BELOW `buildStartedAtMs` even in a correct build (observed:
  // 0.08ms on the CI filesystem), so a `BUILD_START_SLACK_MS` window absorbs
  // that before anything is treated as genuine skew.
  describe('the buildStartedAtMs vs binaryMtimeMs guard (with FS-granularity slack)', () => {
    // RED-FIRST case (a), future-dated buildStartedAtMs: verified red against
    // the unguarded comparison by hand - `threshold = stamp.buildStartedAtMs ??
    // stamp.binaryMtimeMs` alone would pick BUILD_FINISHED + 1e9 as the
    // threshold, and BUILD_FINISHED + 1000 does not exceed that, so the
    // unguarded code calls this fresh. Without the guard, EVERY later edit
    // reads fresh forever - worse than the pre-#554 behavior it replaces. The
    // skew here (1e9 ms) is many orders past BUILD_START_SLACK_MS (2000ms), so
    // this direction is unaffected by the slack.
    it('rejects a future-dated buildStartedAtMs and falls back to binaryMtimeMs', () => {
      const futureStamp: E2EBuildStamp = {
        ...stampWithStart,
        buildStartedAtMs: BUILD_FINISHED + 1_000_000_000,
      };
      expect(sourceStaleReason(BUILD_FINISHED + 1000, BUILD_FINISHED, futureStamp)).toBe(
        'a source file is newer than the binary',
      );
    });

    // RED-FIRST case (b), the rebuild-loop direction: a source strictly OLDER
    // than binaryMtimeMs, paired with the same absurd future buildStartedAtMs.
    // Without the guard this reads newestSource > threshold as false (the
    // source is older than the huge future threshold too), so it happens to
    // stay non-stale by accident here - the real hazard this guard closes is
    // the opposite: a filesystem whose CURRENT time trails buildStartedAtMs
    // would otherwise make buildStartedAtMs > binaryMtimeMs by construction on
    // every stamp that clock ever writes, and every later mtime it stamps
    // reads as newer than a threshold that never advances - a rebuild loop
    // that never converges. Falling back to binaryMtimeMs keeps this input
    // fresh-eligible via the SAME code path a well-formed stamp would use.
    // Unaffected by the slack for the same reason as the case above.
    it('keeps a source older than binaryMtimeMs fresh-eligible despite an absurd future start', () => {
      const futureStamp: E2EBuildStamp = {
        ...stampWithStart,
        buildStartedAtMs: BUILD_FINISHED + 1_000_000_000,
      };
      expect(sourceStaleReason(BUILD_FINISHED - 1000, BUILD_FINISHED, futureStamp)).toBeNull();
    });

    it('rejects buildStartedAtMs equal to 0 (falsy but numeric, not a real timestamp)', () => {
      const zeroStamp: E2EBuildStamp = { ...stampWithStart, buildStartedAtMs: 0 };
      expect(sourceStaleReason(BUILD_FINISHED + 1000, BUILD_FINISHED, zeroStamp)).toBe(
        'a source file is newer than the binary',
      );
    });

    it('accepts buildStartedAtMs exactly equal to binaryMtimeMs (an instant build)', () => {
      const instantStamp: E2EBuildStamp = { ...stampWithStart, buildStartedAtMs: BUILD_FINISHED };
      expect(sourceStaleReason(BUILD_FINISHED - 1000, BUILD_FINISHED, instantStamp)).toBeNull();
      expect(sourceStaleReason(BUILD_FINISHED + 1000, BUILD_FINISHED, instantStamp)).toBe(
        'a source file changed after the build started',
      );
    });

    // The exact CI failure, as a fixture: buildStartedAtMs = 1786499818461,
    // binaryMtimeMs = 1786499818460.9158 (0.0842ms below - a plain float floor
    // from fs.statSync on the CI filesystem). RED against the pre-slack exact
    // `<=` guard: `1786499818461 <= 1786499818460.9158` is false, so that guard
    // rejected this real, non-skewed stamp and silently fell back to the
    // pre-#554 binMtime-only comparison. Threshold must still be BUILD-START-
    // OR-MIN (here, binaryMtimeMs itself, since it is the smaller of the two)
    // and the reason must still say "changed after the build started" - this
    // must NOT be classified as skew.
    it('treats the floored-mtime CI case as within slack, not as clock skew', () => {
      const buildStartedAtMs = 1_786_499_818_461;
      const binaryMtimeMs = 1_786_499_818_460.9158;
      const flooredStamp: E2EBuildStamp = { ...stampWithStart, buildStartedAtMs, binaryMtimeMs };

      // A source landing between the floored mtime and the nominal start must
      // still be caught - min(buildStartedAtMs, binaryMtimeMs) is binaryMtimeMs
      // here, and this reason text is the "trustworthy start" one, not the
      // skewed-fallback one.
      expect(sourceStaleReason(binaryMtimeMs + 1, binaryMtimeMs, flooredStamp)).toBe(
        'a source file changed after the build started',
      );
      // A source strictly before the floored mtime is still fresh-eligible.
      expect(sourceStaleReason(binaryMtimeMs - 1, binaryMtimeMs, flooredStamp)).toBeNull();
    });

    it('accepts buildStartedAtMs exactly BUILD_START_SLACK_MS past binaryMtimeMs (the slack boundary)', () => {
      const SLACK_MS = 2000;
      const stampAtBoundary: E2EBuildStamp = {
        ...stampWithStart,
        buildStartedAtMs: BUILD_FINISHED + SLACK_MS,
      };
      // Within slack: threshold is min(buildStartedAtMs, binaryMtimeMs) =
      // binaryMtimeMs (the smaller), so a source just past it is still caught,
      // with the "trustworthy start" reason.
      expect(sourceStaleReason(BUILD_FINISHED + 1, BUILD_FINISHED, stampAtBoundary)).toBe(
        'a source file changed after the build started',
      );
    });

    it('rejects buildStartedAtMs one ms past the slack boundary', () => {
      const SLACK_MS = 2000;
      const stampPastBoundary: E2EBuildStamp = {
        ...stampWithStart,
        buildStartedAtMs: BUILD_FINISHED + SLACK_MS + 1,
      };
      // Past slack: treated as skew, falls back to binaryMtimeMs with the
      // skewed-fallback reason.
      expect(sourceStaleReason(BUILD_FINISHED + 1000, BUILD_FINISHED, stampPastBoundary)).toBe(
        'a source file is newer than the binary',
      );
    });
  });
});

// Composition seam: `checkBuildFreshness` pushes BOTH the stamp reasons and
// the source-timing reason into one array via `freshnessReasons`. A mutation
// dropping the `sourceStaleReason` call inside it - or dropping it only on the
// fresh-stamp branch - passes every `sourceStaleReason`-only test (that
// function is still correct in isolation) while `checkBuildFreshness` itself
// stops catching the #554 race. These pin the WIRING, not just the halves.
describe('freshnessReasons', () => {
  const BUILD_STARTED = 1_700_000_000_000;
  const BUILD_FINISHED = 1_700_000_060_000;
  const stampWithStart: E2EBuildStamp = {
    ...E2E_STAMP,
    binaryMtimeMs: BUILD_FINISHED,
    buildStartedAtMs: BUILD_STARTED,
  };
  const baseInput = {
    binMtime: BUILD_FINISHED,
    stamp: stampWithStart,
    viteE2E: true,
    e2eFeature: false,
    distIndexHash: stampWithStart.distIndexHash,
    repoRoot: stampWithStart.repoRoot,
  };

  // RED against a `freshnessReasons` whose body is only `return
  // stampStaleReasons(...)` (the sourceReason push deleted): every stamp field
  // matches (a fresh, matching stamp), so that mutant returns [].
  it('surfaces the source-timing reason even when the stamp itself matches perfectly', () => {
    const midBuildEdit = BUILD_STARTED + 30_000;
    const reasons = freshnessReasons({ ...baseInput, newestSource: midBuildEdit });
    expect(reasons).toEqual(['a source file changed after the build started']);
  });

  it('reports both a stamp mismatch and a source-timing reason together', () => {
    const midBuildEdit = BUILD_STARTED + 30_000;
    const reasons = freshnessReasons({ ...baseInput, viteE2E: false, newestSource: midBuildEdit });
    expect(reasons).toHaveLength(2);
    expect(reasons.some((r) => r.includes('VITE_E2E'))).toBe(true);
    expect(reasons.some((r) => r.includes('a source file changed after the build started'))).toBe(true);
  });

  it('returns no reasons when the stamp matches and nothing raced the build', () => {
    expect(freshnessReasons({ ...baseInput, newestSource: BUILD_STARTED - 1000 })).toEqual([]);
  });
});

describe('newestSourceMtime', () => {
  let dir: string;

  beforeEach(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-sources-'));
  });

  afterEach(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('returns 0 for an empty checkout (no tracked inputs present)', () => {
    expect(newestSourceMtime(dir)).toBe(0);
  });

  // Table-driven over the EXPORTED SOURCE_DIRS list itself, not a hand-picked
  // subset: dropping ANY entry from SOURCE_DIRS (public/, src-tauri/vendor,
  // src-tauri/capabilities, ...) makes its own case go red, rather than only
  // the handful an author remembered to write by name.
  describe.each(SOURCE_DIRS.map((dir) => ({ dir })))('tracked dir: $dir', ({ dir: sourceDir }) => {
    it('picks up an edit under this dir', () => {
      const target = path.join(dir, sourceDir);
      fs.mkdirSync(target, { recursive: true });
      fs.writeFileSync(path.join(target, 'touched.txt'), 'x');
      expect(newestSourceMtime(dir)).toBeGreaterThan(0);
    });
  });

  // Table-driven over the EXPORTED SOURCE_FILES list itself: dropping any one
  // of the nine entries (the cargo half - Cargo.toml/.lock/build.rs - and the
  // vite/config half - pnpm-lock.yaml, tsconfig.json, postcss/tailwind
  // configs, .env*) goes red on its own case.
  describe.each(SOURCE_FILES.map((file) => ({ file })))('tracked file: $file', ({ file: sourceFile }) => {
    it('picks up an edit to this file', () => {
      const target = path.join(dir, sourceFile);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.writeFileSync(target, 'x');
      expect(newestSourceMtime(dir)).toBeGreaterThan(0);
    });
  });

  // A file that is NOT a tracked input (nothing vite or cargo reads) must not
  // move the mtime - proves the scan is bounded to the list, not "everything".
  it('ignores a file outside every tracked dir/file', () => {
    fs.writeFileSync(path.join(dir, 'NOTES.txt'), 'scratch');
    expect(newestSourceMtime(dir)).toBe(0);
  });
});

// Pins the wiring between the platform selector and the freshness decision: a
// regression hardcoding e2eFeature:false in checkBuildFreshness's `want` would
// otherwise pass every stamp test and recreate the silent mac session-create
// hang the v2 stamp exists to prevent.
describe('needsE2eFeature', () => {
  const KEY = 'STALAGMIGHT_AGENT_PLATFORM';
  let saved: string | undefined;

  beforeEach(() => {
    saved = process.env[KEY];
  });
  afterEach(() => {
    if (saved === undefined) delete process.env[KEY];
    else process.env[KEY] = saved;
  });

  it('is true when the target platform is darwin', () => {
    process.env[KEY] = 'darwin';
    expect(needsE2eFeature()).toBe(true);
  });

  it('is false when the target platform is linux', () => {
    process.env[KEY] = 'linux';
    expect(needsE2eFeature()).toBe(false);
  });

  it('follows the real platform when no override is set', () => {
    delete process.env[KEY];
    expect(needsE2eFeature()).toBe(process.platform === 'darwin');
  });
});

// The restore guard must recover from an INTERRUPTED feature build (killed
// smoke, ctrl-c) - which leaves churn behind with nothing to time it against -
// while never discarding a real edit to the generated schemas.
describe('isPluginAclChurn', () => {
  const churn = [
    'diff --git a/src-tauri/gen/schemas/macOS-schema.json b/src-tauri/gen/schemas/macOS-schema.json',
    '--- a/src-tauri/gen/schemas/macOS-schema.json',
    '+++ b/src-tauri/gen/schemas/macOS-schema.json',
    '@@ -1,3 +1,4 @@',
    '   "existing": true,',
    '+  "webdriver-automation:allow-element-find",',
    '+  "webdriver-automation:default"',
  ].join('\n');

  it('accepts a diff whose additions are all plugin ACL entries', () => {
    expect(isPluginAclChurn(churn)).toBe(true);
  });

  it('rejects a diff carrying any unrelated addition', () => {
    expect(isPluginAclChurn(`${churn}\n+  "someone-elses:real-permission"`)).toBe(false);
  });

  it('rejects an empty diff (nothing to restore is not churn)', () => {
    expect(isPluginAclChurn('')).toBe(false);
  });

  // The feature build only ADDS entries, so a removal proves a human edited
  // these files - restoring would discard their work.
  it('rejects a diff containing any removal, even when every addition is churn', () => {
    expect(isPluginAclChurn(`${churn}\n-  "someone-elses:deleted-permission"`)).toBe(false);
  });

  it('does not mistake the --- file header for a removal', () => {
    expect(isPluginAclChurn(churn)).toBe(true);
  });

  it('ignores the +++ file header, which is not an added line', () => {
    const headerOnly = ['--- a/x.json', '+++ b/x.json'].join('\n');
    expect(isPluginAclChurn(headerOnly)).toBe(false);
  });
});

// `gitHeadFor` is what both the stamp and sessionCore's session record use, and
// its whole point is the `-C <root>`: the MCP server is launched with no cwd
// pinning, so a HEAD read from the inherited cwd can belong to any repo on the
// machine - or to none.
describe('gitHeadFor', () => {
  it('reads the HEAD of the tree it is pointed at, not the process cwd', () => {
    // A SECOND repo, because vitest's cwd is already this one: asserting only
    // that gitHeadFor(harnessRoot) returns a sha would pass just as happily if
    // the `-C <root>` were dropped and git answered about the cwd.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-otherrepo-'));
    try {
      execFileSync('git', ['init', '-q', dir]);
      execFileSync('git', [
        '-C',
        dir,
        '-c',
        'user.name=t',
        '-c',
        'user.email=t@t',
        'commit',
        '--allow-empty',
        '-m',
        'x',
      ]);
      const expected = execFileSync('git', ['-C', dir, 'rev-parse', '--short', 'HEAD'], {
        encoding: 'utf8',
      }).trim();

      expect(gitHeadFor(dir)).toBe(expected);
      // And it is genuinely a different answer from this tree's, so the `-C`
      // is doing the work rather than coinciding with the cwd.
      expect(gitHeadFor(dir)).not.toBe(gitHeadFor(HARNESS_ROOT_REAL));
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it('returns "unknown" rather than throwing when the path is not a git tree', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-nogit-'));
    try {
      // The tmpdir is not inside any repo here, so the fallback is observable:
      // a session start must survive an unreadable HEAD, not throw out of it.
      expect(() => gitHeadFor(dir)).not.toThrow();
      expect(gitHeadFor(dir)).toBe('unknown');
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe('readBuildStamp', () => {
  let dir: string;

  beforeEach(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-stamp-'));
  });

  afterEach(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  function write(contents: string): string {
    const file = path.join(dir, 'stamp.json');
    fs.writeFileSync(file, contents);
    return file;
  }

  it('returns null for a missing file', () => {
    expect(readBuildStamp(path.join(dir, 'nope.json'))).toBeNull();
  });

  it('returns null for malformed JSON', () => {
    expect(readBuildStamp(write('{ not json'))).toBeNull();
  });

  it('returns null for an unknown schema version', () => {
    expect(readBuildStamp(write(JSON.stringify({ ...E2E_STAMP, v: 4 })))).toBeNull();
  });

  it('returns null for a v2 stamp (cannot name the tree or the bundle - one forced rebuild)', () => {
    const v2 = { ...E2E_STAMP, v: 2 } as Record<string, unknown>;
    delete v2['repoRoot'];
    delete v2['gitHead'];
    delete v2['distIndexHash'];
    expect(readBuildStamp(write(JSON.stringify(v2)))).toBeNull();
  });

  it('returns null for a v3 stamp missing repoRoot, the field the whole defence rests on', () => {
    const noRoot = { ...E2E_STAMP } as Record<string, unknown>;
    delete noRoot['repoRoot'];
    expect(readBuildStamp(write(JSON.stringify(noRoot)))).toBeNull();
  });

  it('returns null when repoRoot is not a string, or is the empty string', () => {
    expect(readBuildStamp(write(JSON.stringify({ ...E2E_STAMP, repoRoot: 7 })))).toBeNull();
    // '' is not a checkout: it would compare unequal to every real root and so
    // rebuild forever, which reads as a broken harness rather than a bad stamp.
    expect(readBuildStamp(write(JSON.stringify({ ...E2E_STAMP, repoRoot: '' })))).toBeNull();
  });

  it('returns null for a v3 stamp missing distIndexHash', () => {
    const noHash = { ...E2E_STAMP } as Record<string, unknown>;
    delete noHash['distIndexHash'];
    expect(readBuildStamp(write(JSON.stringify(noHash)))).toBeNull();
  });

  it('defaults a missing gitHead to "unknown" (informational, never load-bearing)', () => {
    const noHead = { ...E2E_STAMP } as Record<string, unknown>;
    delete noHead['gitHead'];
    expect(readBuildStamp(write(JSON.stringify(noHead)))).toEqual({ ...E2E_STAMP, gitHead: 'unknown' });
  });

  it('returns null for a v1 stamp (predates e2eFeature AND the profile - one forced rebuild)', () => {
    const v1 = { ...E2E_STAMP, v: 1 } as Record<string, unknown>;
    delete v1['e2eFeature'];
    expect(readBuildStamp(write(JSON.stringify(v1)))).toBeNull();
  });

  it('returns null when the required fields have the wrong type', () => {
    expect(readBuildStamp(write(JSON.stringify({ ...E2E_STAMP, viteE2E: 'yes' })))).toBeNull();
    expect(readBuildStamp(write(JSON.stringify({ ...E2E_STAMP, e2eFeature: 'on' })))).toBeNull();
    expect(readBuildStamp(write(JSON.stringify({ ...E2E_STAMP, binaryMtimeMs: null })))).toBeNull();
  });

  it('round-trips a well-formed stamp', () => {
    expect(readBuildStamp(write(JSON.stringify(E2E_STAMP)))).toEqual(E2E_STAMP);
  });

  it('a stamp read back from disk is accepted against its own binary mtime', () => {
    const stamp = readBuildStamp(write(JSON.stringify(E2E_STAMP)));
    expect(stamp).not.toBeNull();
    expect(stampStaleReasons(stamp, wantMatching)).toEqual([]);
  });

  // Issue #554: buildStartedAtMs is a NEW optional field, read leniently like
  // gitHead - a v3 stamp missing it (every stamp written before this fix) must
  // still validate rather than forcing a rebuild on every machine.
  it('round-trips a stamp carrying buildStartedAtMs', () => {
    const withStart = { ...E2E_STAMP, buildStartedAtMs: 1_700_000_000_123 };
    expect(readBuildStamp(write(JSON.stringify(withStart)))).toEqual(withStart);
  });

  it('stays valid when buildStartedAtMs is absent (a stamp written before #554)', () => {
    const stamp = readBuildStamp(write(JSON.stringify(E2E_STAMP)));
    expect(stamp).not.toBeNull();
    expect('buildStartedAtMs' in (stamp as object)).toBe(false);
  });

  // Lenient handling for a malformed value, same shape as gitHead's fallback:
  // the field is DROPPED (undefined), not rejected - a bad buildStartedAtMs
  // must not fail the whole stamp, it should just fall back to the coarser
  // binaryMtimeMs comparison in sourceStaleReason.
  it('drops a non-number buildStartedAtMs rather than rejecting the stamp', () => {
    const bogus = { ...E2E_STAMP, buildStartedAtMs: 'not-a-number' };
    const stamp = readBuildStamp(write(JSON.stringify(bogus)));
    expect(stamp).not.toBeNull();
    expect('buildStartedAtMs' in (stamp as object)).toBe(false);
  });
});

// `builtFromFor` is what a session reports on the `build: "skip"` path, where it
// is the only thing between the caller and a foreign binary. It must therefore
// answer for the binary that is THERE, not for whatever the stamp remembers.
describe('builtFromFor', () => {
  let dir: string;
  let stampFile: string;
  let binaryFile: string;

  beforeEach(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-builtfrom-'));
    stampFile = path.join(dir, 'stamp.json');
    binaryFile = path.join(dir, 'stalagmight');
    fs.writeFileSync(binaryFile, 'not really a binary');
  });

  afterEach(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  /** Write a stamp that vouches for `binaryFile` as it stands right now. */
  function stampFor(binary: string): void {
    const stamp: E2EBuildStamp = { ...E2E_STAMP, binaryMtimeMs: fs.statSync(binary).mtimeMs };
    fs.writeFileSync(stampFile, JSON.stringify(stamp));
  }

  it('reports the builder when the stamp vouches for the binary on disk', () => {
    stampFor(binaryFile);
    expect(builtFromFor('debug', stampFile, binaryFile)).toEqual({
      repoRoot: E2E_STAMP.repoRoot,
      gitHead: E2E_STAMP.gitHead,
    });
  });

  it('says nothing once the binary is relinked behind the stamp', () => {
    stampFor(binaryFile);
    // What `./scripts/dev.sh` or a plain `pnpm tauri build` does: the binary is
    // replaced in place and the old stamp is left sitting next to it. The
    // stamper did not build THAT binary, so claiming its identity would be a
    // confident lie on the one path that has nothing else to check.
    const later = new Date(Date.now() + 60_000);
    fs.utimesSync(binaryFile, later, later);

    expect(builtFromFor('debug', stampFile, binaryFile)).toBeNull();
  });

  it('says nothing when there is no stamp at all', () => {
    expect(builtFromFor('debug', path.join(dir, 'absent.json'), binaryFile)).toBeNull();
  });

  it('says nothing when the binary is gone', () => {
    stampFor(binaryFile);
    fs.rmSync(binaryFile);
    expect(builtFromFor('debug', stampFile, binaryFile)).toBeNull();
  });
});

// Pre-PR review finding: `sourceStaleReason`'s guard is only as good as what
// the real writer actually produces. This pins the writer's OWN invariant -
// buildStartedAtMs within BUILD_START_SLACK_MS of binaryMtimeMs - on a stamp
// `writeBuildStamp` itself wrote, not on a hand-built fixture, so a future
// change to the write order (e.g. moving `Date.now()` to capture
// buildStartedAtMs AFTER the binary is written, or introducing a delay large
// enough to matter) shows up here even though every `sourceStaleReason` test
// above still passes in isolation.
//
// Second review round: the assertion here is the ONE that actually failed in
// CI - `fs.statSync` returned `binaryMtimeMs` floored 0.08ms below the
// `buildStartedAtMs` captured moments earlier, which is legitimate filesystem
// mtime truncation, not a bug in the writer. The assertion below checks the
// GUARDED relation (within slack), not the exact `<=` that CI proved false
// for a correct build.
describe('writeBuildStamp', () => {
  let dir: string;
  let binaryFile: string;
  let stampFile: string;

  beforeEach(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-writestamp-'));
    binaryFile = path.join(dir, 'stalagmight');
    stampFile = path.join(dir, 'stamp.json');
  });

  afterEach(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('records a buildStartedAtMs within BUILD_START_SLACK_MS of the binaryMtimeMs it reads off the same binary', () => {
    // Mirrors buildApp's real order: capture the start time, THEN produce the
    // binary (the build), THEN stamp. RED if that order were reversed (stamp
    // captured long after the binary write) - the assertion below would fail
    // on real output, not a synthetic case.
    const SLACK_MS = 2000; // Mirrors build.ts's BUILD_START_SLACK_MS (FAT worst case).
    const buildStartedAtMs = Date.now();
    fs.writeFileSync(binaryFile, 'not really a binary');

    writeBuildStamp(
      {
        viteE2E: true,
        e2eFeature: false,
        viteMode: 'production',
        mode: 'debug',
        features: '',
        buildStartedAtMs,
      },
      undefined,
      binaryFile,
      stampFile,
    );

    const written = readBuildStamp(stampFile);
    expect(written).not.toBeNull();
    expect(written?.buildStartedAtMs).toBe(buildStartedAtMs);
    // NOT an exact `<=`: filesystem mtime granularity can legitimately floor
    // binaryMtimeMs a fraction below buildStartedAtMs (the CI failure this
    // guards against) even though the writer's order is correct.
    expect(written?.buildStartedAtMs).toBeLessThanOrEqual((written?.binaryMtimeMs ?? 0) + SLACK_MS);
    // And the guard in sourceStaleReason accepts what the real writer wrote.
    expect(sourceStaleReason(buildStartedAtMs - 1, written?.binaryMtimeMs ?? 0, written)).toBeNull();
  });

  it('reports a stamp it could not write by its errno code, never by the path (#576 W51)', () => {
    // A directory where the stamp file should be: the write fails with EISDIR.
    const blocked = path.join(dir, 'qzCaveA');
    fs.mkdirSync(blocked);
    const lines: string[] = [];
    writeBuildStamp(
      { viteE2E: true, e2eFeature: false, viteMode: 'production', mode: 'debug', features: '', buildStartedAtMs: 0 },
      (line) => lines.push(line),
      binaryFile,
      blocked,
    );
    expect(lines).toEqual(['Warning: could not write E2E build stamp (EISDIR); next start will rebuild.']);
  });
});

describe('hashDistIndex', () => {
  let dir: string;

  beforeEach(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-dist-'));
  });

  afterEach(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('returns "" when there is no bundle to hash', () => {
    expect(hashDistIndex(dir)).toBe('');
  });

  it('hashes dist/index.html, stably and by content', () => {
    fs.mkdirSync(path.join(dir, 'dist'), { recursive: true });
    const index = path.join(dir, 'dist', 'index.html');
    fs.writeFileSync(index, '<!doctype html><script src="/assets/index-abc123.js"></script>');

    const first = hashDistIndex(dir);
    // A literal, cross-checked against coreutils - `printf '%s' <the string> |
    // md5sum` gives this same digest. That is the actual claim being pinned:
    // The retired scripts/wdio.sh computed .e2e-build-marker with the shell tool, so the two
    // tiers only agree about "this bundle" if both digests are plain md5 of the
    // file bytes.
    expect(first).toBe('d14f063eb4fdcd3e0a81ab448bae9fe9');
    expect(hashDistIndex(dir)).toBe(first);

    // A vite rebuild renames the chunk, and index.html references it by name.
    fs.writeFileSync(index, '<!doctype html><script src="/assets/index-def456.js"></script>');
    expect(hashDistIndex(dir)).not.toBe(first);
  });
});

// The two halves of the shared cargo target dir: this branch and the fuzzer
// branch both write `v: 3`, with field sets that differ by `gitHead`. Each
// reader must accept the other's stamps, or the two would rebuild over each
// other on every start - the exact thrash the stamp exists to prevent.
describe('cross-schema readability', () => {
  let dir: string;

  beforeEach(() => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-cross-'));
  });

  afterEach(() => {
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('reads a stamp shaped like the sibling branch writes (no gitHead)', () => {
    const theirs = {
      v: 3,
      mode: 'debug',
      features: '',
      viteE2E: true,
      e2eFeature: false,
      viteMode: 'production',
      binaryMtimeMs: E2E_STAMP.binaryMtimeMs,
      distIndexHash: E2E_STAMP.distIndexHash,
      repoRoot: '/repo/b',
      builtAtMs: E2E_STAMP.builtAtMs,
      builtAtIso: E2E_STAMP.builtAtIso,
    };
    const file = path.join(dir, 'theirs.json');
    fs.writeFileSync(file, JSON.stringify(theirs, null, 2));

    const parsed = readBuildStamp(file);
    expect(parsed).not.toBeNull();
    expect(parsed?.repoRoot).toBe('/repo/b');
    expect(parsed?.gitHead).toBe('unknown');
    // And it still decides: a foreign root is stale, HEAD or no HEAD.
    expect(stampStaleReasons(parsed, wantMatching)).toHaveLength(1);
  });

  it('writes a stamp the sibling branch validation would accept', () => {
    // Their rules, replicated rather than imported (their tree is not ours to
    // depend on): v3, every load-bearing field present and typed, repoRoot
    // non-empty. `gitHead` is an extra key, which JSON validation ignores.
    const acceptedByTheirRules = (raw: unknown): boolean => {
      if (typeof raw !== 'object' || raw === null) return false;
      const c = raw as Record<string, unknown>;
      return (
        c['v'] === 3 &&
        (c['mode'] === 'debug' || c['mode'] === 'release') &&
        typeof c['features'] === 'string' &&
        typeof c['viteE2E'] === 'boolean' &&
        typeof c['e2eFeature'] === 'boolean' &&
        typeof c['binaryMtimeMs'] === 'number' &&
        typeof c['distIndexHash'] === 'string' &&
        typeof c['repoRoot'] === 'string' &&
        c['repoRoot'] !== ''
      );
    };

    expect(acceptedByTheirRules(JSON.parse(JSON.stringify(E2E_STAMP)))).toBe(true);
    // Guard the guard: the same predicate must reject a v2 stamp, or it would
    // "accept" anything and prove nothing.
    expect(acceptedByTheirRules({ ...E2E_STAMP, v: 2 })).toBe(false);
  });
});

describe('build modes', () => {
  it('keeps one stamp per profile, beside the binary it vouches for', () => {
    const debugStamp = buildStampPathFor('debug');
    const releaseStamp = buildStampPathFor('release');

    expect(debugStamp).not.toBe(releaseStamp);
    expect(path.dirname(debugStamp).endsWith('debug')).toBe(true);
    expect(path.dirname(releaseStamp).endsWith('release')).toBe(true);
  });

  it('treats the other profile as stale rather than reusing its binary', () => {
    const reasons = stampStaleReasons(E2E_STAMP, {
      viteE2E: true,
      e2eFeature: featuresFor('release').includes('e2e'),
      binaryMtimeMs: E2E_STAMP.binaryMtimeMs,
      distIndexHash: E2E_STAMP.distIndexHash,
      repoRoot: E2E_STAMP.repoRoot,
      mode: 'release',
      features: 'e2e',
    });

    // Three, not two: a debug stamp is the wrong profile, was built with the
    // wrong feature set, and (on a platform where the release profile is what
    // asks for `e2e`) carries the wrong e2eFeature bit.
    expect(reasons).toHaveLength(3);
    expect(reasons[0]).toContain('for the debug profile but this start needs release');
    expect(reasons[1]).toContain('features "(none)" but this start needs "e2e"');
    expect(reasons[2]).toContain('e2e cargo feature');
  });

  it('accepts a release stamp for a release start', () => {
    // e2eFeature true, because that is what a release build WRITES: `buildApp`
    // stamps the union it passed, and the release profile asks for `e2e`. The
    // old fixture left it false - a stamp no build could produce - which is how
    // the comparison stayed vacuous.
    const releaseStamp: E2EBuildStamp = {
      ...E2E_STAMP,
      mode: 'release',
      features: 'e2e',
      e2eFeature: true,
    };

    expect(
      stampStaleReasons(releaseStamp, {
        viteE2E: true,
        e2eFeature: featuresFor('release').includes('e2e'),
        binaryMtimeMs: releaseStamp.binaryMtimeMs,
        distIndexHash: releaseStamp.distIndexHash,
        repoRoot: releaseStamp.repoRoot,
        mode: 'release',
        features: 'e2e',
      }),
    ).toEqual([]);
  });

  it('rejects a v1 stamp, which cannot say which profile it describes', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-stamp-'));
    const file = path.join(dir, 'stamp.json');
    fs.writeFileSync(file, JSON.stringify({ ...E2E_STAMP, v: 1 }));

    expect(readBuildStamp(file)).toBeNull();
  });

  it('gives release the e2e feature and debug none', () => {
    // The body of debug_trigger_panic is #[cfg(any(debug_assertions, feature =
    // "e2e"))], so release needs the feature to keep the crash-oracle
    // self-test; debug must stay featureless so the DEFAULT build is unchanged.
    expect(FEATURES_FOR_MODE.debug).toEqual([]);
    expect(FEATURES_FOR_MODE.release).toEqual(['e2e']);
  });

  it('accepts the stamp a build of the SAME profile just wrote (no rebuild loop)', () => {
    // The producer/consumer round trip, which the two cases above cannot make:
    // they fed the stamp's own e2eFeature back in as the want, so that half of
    // the comparison was `x !== x`. Production passes what the PLATFORM plus the
    // PROFILE ask for, and when those two disagreed - Linux release, where the
    // profile wants `e2e` and the platform does not - every start rebuilt.
    for (const mode of ['debug', 'release'] as const) {
      const features = featuresFor(mode);
      const asBuilt: E2EBuildStamp = {
        ...E2E_STAMP,
        mode,
        features: features.join(','),
        e2eFeature: features.includes('e2e'),
        // What `writeBuildStamp` records, and what `checkBuildFreshness` then
        // compares against: both go through the same realpath of the harness
        // tree, so a build in THIS tree must not read as a foreign build.
        repoRoot: HARNESS_ROOT_REAL,
      };

      expect(
        stampStaleReasons(asBuilt, {
          viteE2E: asBuilt.viteE2E,
          e2eFeature: featuresFor(mode).includes('e2e'),
          binaryMtimeMs: asBuilt.binaryMtimeMs,
          distIndexHash: asBuilt.distIndexHash,
          repoRoot: HARNESS_ROOT_REAL,
          mode,
          features: features.join(','),
        }),
        `${mode}: a freshly written stamp must satisfy its own freshness check`,
      ).toEqual([]);
    }
  });

  it('names only features the crate actually declares', () => {
    // The assertion above compares a literal with itself: it fails on an edit to
    // the map and on nothing else. The hazard it reads as covering is the other
    // side - the cargo feature being renamed or dropped - which would leave the
    // map green while EVERY release start dies at `cargo build`. Only reading
    // Cargo.toml can catch that.
    const manifest = fs.readFileSync(
      path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'src-tauri', 'Cargo.toml'),
      'utf8',
    );
    const featuresBlock = /\n\[features\]\n([\s\S]*?)(?=\n\[|$)/.exec(manifest)?.[1] ?? '';
    const declared = new Set(
      [...featuresBlock.matchAll(/^\s*([A-Za-z][\w-]*)\s*=/gm)].map((match) => match[1] ?? ''),
    );

    expect(declared.size).toBeGreaterThan(0);
    for (const mode of ['debug', 'release'] as const) {
      for (const feature of FEATURES_FOR_MODE[mode]) {
        expect(declared.has(feature), `src-tauri/Cargo.toml declares no [features] entry "${feature}"`).toBe(
          true,
        );
      }
    }
  });

  it('rejects a stamp whose mode or features field is not a valid one', () => {
    // Both fields are new, and both are load-bearing: a stamp that survives
    // validation with a bad `mode` lets a RELEASE binary satisfy a debug start,
    // which silently drops the debug_assert oracles the harness relies on.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-stamp-'));
    const cases: Record<string, unknown>[] = [
      { ...E2E_STAMP, mode: 'profile' },
      { ...E2E_STAMP, mode: undefined },
      { ...E2E_STAMP, features: 3 },
      { ...E2E_STAMP, features: undefined },
    ];
    for (const [index, candidate] of cases.entries()) {
      const file = path.join(dir, `stamp-${index}.json`);
      fs.writeFileSync(file, JSON.stringify(candidate));
      expect(readBuildStamp(file), JSON.stringify(candidate)).toBeNull();
    }
  });
});
