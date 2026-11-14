# Agent harness

An LLM-agent harness for driving the **real** running Stalagmight app: testing,
stress testing, bug discovery, and plain interactive use. An MCP stdio server
owns a persistent WebdriverIO session against the Tauri debug binary (headless
Xvfb on Linux), exposing tools for clicking, typing, screenshots, store-state
reads, IPC invokes, stress scenarios, and a problem sweep. Every tool call is
recorded to a JSONL action log that a generator turns into a runnable WDIO
replay spec and a repro report.

For the agent-facing workflow, see the `app-harness` skill
(`.claude/skills/app-harness/SKILL.md`). This document is the reference.

## Runtime: the repo-pinned Node

The harness runs on whatever `.nvmrc` pins (Node 24), like the rest of the repo.
`nvm use` is enough; there is no separate harness toolchain.

This used to say the harness required Node 22 and would fail on 23+ with
`UND_ERR_INVALID_ARG` at `POST /session`, because webdriverio built an `Agent`
from its bundled undici and handed it to Node's built-in `fetch`, so the two
undici majors had to agree. That mechanism is not what the code does. `webdriver`
9.23.0 imports `fetch` from undici alongside `Agent` (`build/node.js:1614`) and
calls that fetch (`:1986`), so the two undici majors no longer have to agree
about `fetch` itself. The DISPATCHER is a different story: `getDispatcher` reads
`getGlobalDispatcher()` (`:2001`) and RETURNS whatever sits in that shared slot
unless it is a plain `Agent`/`MockAgent` (`:2002-2003`), so a foreign dispatcher
does reach webdriver's own fetch. On Node >= 26 the builtin undici rewrites the
shared slot (`Symbol.for('undici.globalDispatcher.1')`) with a
`Dispatcher1Wrapper` on the first global fetch, webdriver's undici-6 request is
then routed through undici-8 internals, and every WebDriver command dies with
`UND_ERR_INVALID_ARG` before reaching tauri-driver.
`e2e/agent/lib/undici-dispatcher-pin.ts` - the first import in `session.ts`,
`sessionMac.ts` and `createSession.ts` - patches that copy's
`getGlobalDispatcher` to return a stable plain `Agent` whenever the slot holds
anything else, so webdriver falls through to building its own per-session
`Agent`. Inert on Node 22/24, and best-effort: a resolution failure leaves
webdriver untouched. The global-`fetch` variant is `request/web.ts`, the browser
path, which this harness does not use.

Checked rather than assumed: driving `remote()` through a full session lifecycle
(`POST /session` -> `GET .../window` -> `DELETE`) against a stub WebDriver server
succeeds identically on Node 22 (built-in undici 6.21.2) and Node 24 (undici
7.29.0). The claim also predates nothing - `webdriverio` has been `^9.23.0` since
the harness was written, so this was never a description of a version we no
longer ship.

That probe is committed as `pnpm agent:http-probe`, so this is one command to
re-check after a webdriverio or Node bump rather than something to re-derive. It
runs anywhere, macOS included, because the stub replaces the driver.

That probe covers only the HTTP layer, so the full stack was checked separately on
Linux, where it actually runs. On the self-hosted runner under Node 24.18.1
(built-in undici 7.29.0), `pnpm agent:smoke` built the debug binary, brought up
tauri-driver plus WebKitWebDriver on an Xvfb display, drove the real app and
passed all 13 checks - including the crash oracle (a thread panic wrote its marker
and `PANIC` ring line while the app stayed responsive) and process-group teardown
with both ports released. Whole job: 7 minutes, of which 4m34s was the build.

That is one green run, on one Node patch, on one Linux box - enough to retire the
claim that Node 22 was required, not enough to call every layer Node-agnostic
forever. `session.ts` still warns (advisorily) when the running major differs from
`.nvmrc`, because the verified envelope is only {22, 24} and `node@26` already
exists.

Reproduce with the `Agent Harness (on-demand)` workflow (`workflow_dispatch`). It
runs on the self-hosted runner, so it consumes no *included* Actions minutes - but
that runner also serves the merge gate, and `ci.yml`'s picker spills `check` to
paid hosted runners whenever it is busy, so dispatching this during a merge window
is not free in practice. The runner image carries webkit's `-dev` packages for
`cargo test` but not the WebDriver daemon, so the job installs `webkit2gtk-driver`
and `tauri-driver` (xvfb is usually already there) - about 40s combined.

If `app_start` ever does fail with `UND_ERR_INVALID_ARG`, suspect `HTTP_PROXY` /
`NO_PROXY` in the environment first - the Node >= 26 shared-dispatcher cause is
already pinned out by `undici-dispatcher-pin.ts` above - and record any new cause
here. `agent:http-probe` drops those variables for its own loopback stub
(webdriver snapshots them at module init and would otherwise route a 127.0.0.1
request through the proxy, then blame the driver), so a proxy-caused failure shows
up in `app_start` rather than in the probe.

## macOS

Since #421 the harness also runs on macOS, through a different WebDriver stack:
`createSession()` picks `MacSession` (`e2e/agent/sessionMac.ts`), which drives
`tauri-wd` (`cargo install --git
https://github.com/FumblingBarbell/tauri-webdriver tauri-webdriver-automation`
installs the `tauri-wd` binary onto PATH) against the in-app
`tauri-plugin-webdriver-automation` server, compiled in by the darwin build arm
(`pnpm tauri build --debug --no-bundle -- --features e2e`; the E2E build stamp
records the feature so a plugin-less binary reads as stale). The feature build
regenerates `src-tauri/gen/schemas/*.json` with the plugin's ACL entries;
`buildApp` restores them when they were clean before the build, or when their
only changes are provably the plugin's ACL entries, and warns instead of
clobbering when the diff carries anything else - never commit them in
the feature shape. `e2e/mac-app-wrapper.mjs` sits between tauri-wd and the
binary exactly as in the legacy WDIO suite.

Platform differences that are contracts, not bugs (full rationale + probe
evidence: `docs/plans/2026-08-05_macos-agent-harness.md`):

- **Always headed.** macOS has no Xvfb; `headed: false` is coerced (the start
  result carries `headedForced: true`) and the window is parked on the RIGHT
  half of the screen so it can share the desktop with a human. Override with
  `STALAGMIGHT_AGENT_WINDOW_RECT="x,y,w,h"`.
- **Input is synthetic.** The plugin dispatches JS events in the webview
  (`isTrusted: false`). Everything testid-driven works; native menu
  accelerators do not - `press_keys` with `Meta` returns a `caveat` saying so.
- **`type_text` on an INPUT/TEXTAREA selector writes through React's
  controlled-input path on this route.** Observed: the driver's `setValue()`
  here lands the DOM value (a plain read-back matches) while React's
  onChange never fires, so `type_text` instead runs an in-page script
  (`e2e/agent/lib/reactInput.ts`) that writes through the prototype value
  setter and dispatches a real `input`/`change` event, which React does see.
  Scope: only free-text INPUT types (`text`/`search`/`url`/`tel`/`email`/
  `password`/`number`) and TEXTAREA - a checkbox/radio/date/color/file/etc.
  reports `reason: 'not-a-text-input'` rather than certifying a string write
  that never touched its actual state; a SELECT or anything else reports
  `reason: 'unsupported-element'`; disabled/readOnly reports
  `reason: 'not-editable'`; none of these are written to, and `typed: false`
  suppresses `submit`'s Enter (it would otherwise act on whatever already
  had focus, not the text that failed to land). A `testid`/`selector`-less
  call still types via native driver keys. `verified` is read back after a
  tick so it reflects the app's own re-render, not just the synchronous
  write - it can legitimately be `false` when the app clamps or rejects a
  value. The write also mirrors the target's `maxLength` the way real typing
  would (truncating before writing, reporting `truncated: true`), rather
  than certifying a longer value a user could never have typed. A
  `clear: true` call fires an intermediate empty-string `input` event before
  the target text, which any field-level side effect on empty will observe
  (#531).
- **`press_keys(['Escape'])` gets a synthetic fallback on this route, and
  nothing else does.** Observed: a lone `['Escape']` call produces no effect
  through the native WebDriver key path here (no error, no app response); a
  manually dispatched `KeyboardEvent('keydown'/'keyup', {key: 'Escape'})` on
  the focused element (or the app's mounted root, if nothing is focused)
  does, and darwin targets do this automatically, marking the result
  `syntheticEscape: true`. This is scoped to that exact one-key shape - a
  sequence with `'Escape'` mixed into other keys is unconfirmed on this route
  and carries a `caveat` explaining so rather than silently guessing.
  `type_text`'s own `submit: true` gets the analogous synthetic Enter
  automatically (same broken native path as Escape); a bare `press_keys`
  call for `Enter` or any other key does not (#531).
- **Screenshots are DOM-only.** The plugin rasterizes the DOM, so canvas/WebGL
  (the survey map) is blank; the `screenshot` result carries `domOnly: true`.
- **Teardown is pidfile-based.** tauri-wd SIGKILLs the wrapper on
  `delete_session`, orphaning the app; the wrapper records the real app pid
  into the session pidfile and `MacSession.stop()` reaps it (comm-verified,
  `stalagmight`/`tauri-wd` only).
- **Eval scripts** must finish inside the plugin's 30 s execute-async timeout
  and never resolve to an object carrying both `error` and `message` keys (the
  plugin coerces that shape into a script error). See `e2e/agent/lib/page.ts`.

`pnpm agent:smoke` and `pnpm agent:mcp:smoke` run on macOS unchanged (verified
2026-08-05: 13/13 and full MCP pass on an M1). Real trusted input and real
pixels come from the permission-gated Tier 2 tools under `e2e/agent/os/`,
registered on darwin only: `os_grant`, `os_click`, `os_keys` (real modifier
flags, so Cmd+S reaches the menu bar) and `os_screenshot` (`screencapture` of
the app window). `os_grant` is deliberately never allowlisted, so invoking it
raises a real permission prompt - the human, not the model, decides that a
session may drive the desktop - and the grant lasts until the MCP server exits.
Two TCC permissions gate the machine side: Accessibility for input, Screen
Recording for capture. Both belong to the process that spawns the helper (your
terminal or the editor running the MCP server), never to the helper binary or
to the app, and both fail silently into plausible-looking evidence when denied,
which is why preflight rides every result. Full contract:
[`e2e/agent/os/README.md`](../e2e/agent/os/README.md).

## Architecture

```
MCP client --stdio--> e2e/agent/mcp/server.ts  (tsx, @modelcontextprotocol/sdk)
                        |- AppSession (e2e/agent/session.ts): state machine + command mutex
                        |    |- Xvfb (only if no DISPLAY)
                        |    |- tauri-driver --port 4544 --native-port 5544
                        |    |    -> WebKitWebDriver -> stalagmight debug binary
                        |    |       (isolated HOME/XDG, STALAGMIGHT_TEST_HOOKS=1, RUST_BACKTRACE=1)
                        |    |- webdriverio remote() ('wdio:enforceWebDriverClassic': true)
                        |- Oracles (cheap sweep per mutating action; full sweep on demand)
                        |- Recorder -> .agent-artifacts/sessions/<id>/
                             actions.jsonl, screenshots/, state/, findings/, pids, tauri-driver.log
```

All WebDriver traffic is serialized through a per-session command mutex, so a
sweep never interleaves with a tool call. The session is stateless at the tool
boundary (fresh element resolution every call) to survive WebKit stale-ref
hazards over long runs.

## Setup

- MCP config: `.mcp.json` registers server `stalagmight-app`
  (`pnpm exec tsx e2e/agent/mcp/server.ts`). In Claude Code, `/mcp` shows it.
- Direct scripts (all on the repo-pinned Node, `nvm use`):
  - `pnpm agent:smoke` - session-core smoke (build -> launch -> drive -> stop).
  - `pnpm agent:mcp:smoke` - MCP-layer smoke (spawns the server, round-trips
    every tool through the protocol, self-tests the crash oracle).
  - `pnpm agent:scenarios` - runs all 10 gated stress scenarios once at small n.
    The other three (`import-dialog-fuzz`, `export-roundtrip-fuzz`,
    `fractal-stress`) are excluded from the gate and reported as SKIP; there is
    no expected-red list.
  - `pnpm agent:scenario <name> [--seed N] [--iterations M] [--release]` - runs
    ONE scenario at a chosen seed/n, then a full sweep. The one to reach for when
    a seeded scenario needs re-running: a fuzzer repro, or a soak at a bigger n
    than the gate would tolerate.
  - `pnpm agent:fuzz-matrix [--fixtures a,b] [--seeds 1..8|1,3,7] [--workers K]
    [--iterations M] [--release]` - runs `undo-redo-fuzz` across a (fixture x seed) matrix,
    K child processes at a time, and prints a results table, every failure's
    repro line, merged metric totals and the observed speedup. Each worker slot
    gets its own port block (`STALAGMIGHT_AGENT_BASE_PORT = 4544 + 20*slot`) and
    its own single-entry Xvfb pool (`STALAGMIGHT_AGENT_DISPLAYS`), so `K` is
    hard-capped at 5 - the size of the display pool. Pinning the pool spends the
    retries the default 5-display walk would have, so entries are validated
    (`:N`, non-empty) and a pinned display that will not start is a hard error:
    the session never falls back to the real display, where the fuzzer would
    drive a visible window on the user's desktop for the length of the run. A
    cell GENERATES its own
    moves, so `STALAGMIGHT_FUZZ_REPLAY` / `_TRACE` are dropped from the child
    environment rather than inherited (an exported replay path would otherwise
    make every cell replay one trace and report a green matrix for coverage
    nothing ran). `--release=0` means what it says: the flag takes an optional
    `1/0`, `true/false`, `yes/no`, `on/off` value. `--axis KEY=v1,v2` is the
    optional third dimension; it accepts `STALAGMIGHT_*` keys only (the children
    run cargo and launch the app, so a matrix flag does not get to set `PATH` or
    `LD_PRELOAD`) and REFUSES the six the matrix assigns per cell -
    `STALAGMIGHT_FUZZ_FIXTURE`, `STALAGMIGHT_AGENT_BASE_PORT`,
    `STALAGMIGHT_AGENT_DISPLAYS`, `STALAGMIGHT_AGENT_RELEASE`,
    `STALAGMIGHT_FUZZ_REPLAY`, `STALAGMIGHT_FUZZ_TRACE`. An axis over the
    fixture would mislabel every row of the results table; one over the port
    block or the display would put K concurrent sessions on one of each, which
    is the race the slot assignment exists to remove. The refusal names the flag
    that owns the key, because silently reordering would ignore what was asked
    for. Parsing lives in `e2e/agent/lib/fuzzMatrixArgs.ts` so a unit test can
    assert the refusal list is the same list the per-cell environment writes.
  - `pnpm agent:mcp` - run the server standalone (for an external MCP client).
  - `pnpm agent:bridge <dir>` - file-based access to the same server for plain
    shells and subagents without an MCP client: append JSON tool calls to
    `<dir>/requests.jsonl`, read responses from `<dir>/responses.jsonl`
    (protocol documented in `e2e/agent/mcp/bridge.ts`).
- Agent definitions: `.claude/agents/app-tester.md` (Opus; end-to-end feature
  testing playbook: happy path, edge cases, UI at multiple window sizes and
  zoom levels, per-issue code analysis, structured report) and
  `.claude/agents/code-analyst.md` (Opus, read-only; per-issue root-cause
  analysis it spawns).

## Build profile: debug by default, release opt-in

The harness drives the **debug** binary unless `STALAGMIGHT_AGENT_RELEASE=1` is
set (`--release` on `agent:scenario` and `agent:fuzz-matrix` sets it for their
children). That default is a deliberate trade, not inertia.

**What release costs you.** `debug_assertions` is off, so these stop running:

- Every `debug_assert!` in a live path - `core/state/command_comment.rs:71`,
  four in `core/loading.rs`, `models/project/reverse.rs:379`,
  `models/project/classify.rs:244`, `core/state/undo_restore.rs:264`,
  `core/state/speleodb_pull.rs:1685`, `persistence/sqlite.rs:519`. Each is an
  invariant check the app performs on itself during exactly the mutations a
  fuzzer drives.
- `Project::debug_assert_merge_alignment` (`models/project/offsets.rs:546`), whose
  whole body is `#[cfg(debug_assertions)]`. In debug it emits a `tracing::error!`
  when a merge ends up more than 5 cm out of alignment inside a single command;
  that line rides the ring buffer into the harness's error-log oracle. In release
  the check does not exist, so that class of drift becomes invisible.
- Rust's debug-only integer-overflow panics: an overflow that aborts loudly in
  debug wraps silently in release.
- `[profile.release]` sets `strip = true`, so a crash marker's
  `Backtrace::force_capture()` has no symbol names.

In other words a debug build is itself an oracle, and the release build trades
several of them for speed. Reach for release when you are sweeping seeds for
throughput; keep debug for the run whose result you intend to trust.

**What release keeps.** The release harness build passes `--features e2e`, the
repo's own name for this build. That keeps the BODY of `debug_trigger_panic`
compiled in (the `#[cfg(any(debug_assertions, feature = "e2e"))]` sits on the
body blocks, not on the command - the command stays registered in every build so
the invoke-handler and telemetry-registry contracts are build-invariant, see
`src-tauri/src/commands/debug.rs`), so the crash-oracle self-test still works
without `debug_assertions`. The panic hook, the crash marker and the ring buffer
are not feature-gated and behave identically. `window.__E2E_TEST__` is gated on
`import.meta.env.DEV || MODE === 'test' || VITE_E2E === '1'`, and the frontend leg
is the same `pnpm build` in both profiles, so `VITE_E2E=1` supplies the hooks in a
production-mode vite build too; `waitForE2EHooks` fails the start loudly, naming
the profile and the binary path, if they are ever missing. The debug build passes
no features, so the DEFAULT build is byte-for-byte what it always was.

**What release also brings, which is not only a subtraction.** `e2e` enables
`tauri-plugin-webdriver-automation` (`src-tauri/Cargo.toml`), and `lib.rs`
registers it under `#[cfg(feature = "e2e")]`. So the two profiles differ by a
PLUGIN as well as by the compiled-out assertions above, and "release is debug
minus the `debug_assert!`s" is not the whole truth: a finding that reproduces in
one profile and not the other has a second candidate explanation, and the
comparison to reach for is the same seed in the same profile. (The WDIO tier
builds its DEBUG binary with `--features e2e` for this reason; the agent
harness deliberately does not, to leave its default build unchanged.)

**Cost.** One-time cold release build: about 3.5 minutes wall clock on the
self-hosted runner (`Finished release profile [optimized] in 2m 08s` plus the
vite leg and the link), against roughly 4.5 minutes for debug. Both profiles are
cached independently under `$CARGO_TARGET_DIR`, so switching back and forth does
not rebuild.

## Build freshness + the E2E build stamp

`app_start(build: "auto")` rebuilds when either signal says stale
(`e2e/agent/lib/build.ts`):

1. **mtimes** - any file under `src/`, `src-tauri/src/`, `src-tauri/crates/`,
   `src-tauri/capabilities/`, `src-tauri/vendor/` (the vendored `compass_data`
   crate the parsers crate path-depends on - it feeds the binary despite being
   excluded from the cargo workspace for lint-gate purposes only), or `public/`
   (not present in this checkout today; listed so the scan picks it up the
   moment one exists - forward-proofing, not a closed gap), or one of
   `tauri.conf.json` / `src-tauri/Cargo.toml` / `src-tauri/Cargo.lock` /
   `src-tauri/build.rs` / `package.json` / `pnpm-lock.yaml` / `index.html` /
   `vite.config.ts` / `tsconfig.json` / `postcss.config.js` / `tailwind.config.js` /
   `.env` / `.env.local` / `.env.production` / `.env.production.local` (vite's
   full mode-aware load order for `pnpm build`, which defaults to production
   mode) / `.cargo/config.toml` / `.cargo/config` / `src-tauri/.cargo/config.toml` /
   `src-tauri/.cargo/config` (cargo's config hierarchy, read from the build cwd
   upward and shaping the binary via target dir / rustflags / profile
   overrides - commonly local-only and uncommitted, which is exactly why the
   freshness scan rather than `git status` has to see them) is newer than the
   threshold described below (the build's start time when a stamp has one, the
   binary's own mtime otherwise). This list is the input surface of the WHOLE
   binary pipeline - vite's frontend leg and cargo's backend leg - not the
   tracked source tree in general, and it is a documented heuristic rather than a
   derivation from either build tool's own dependency graph - issue #554
   widened it (the cargo half, the lockfile, the config files, `public/`) on
   the principle that a green run over the wrong bundle is worse than a red
   one over the right question, so the list errs toward over-listing.
2. **E2E build stamp** - `buildApp` writes
   `$CARGO_TARGET_DIR/<profile>/.stalagmight-e2e-build-stamp.json` after a
   successful build, recording the profile, its cargo features, `viteE2E`, the
   vite mode, the mtime of the binary it produced, `distIndexHash` (the content
   hash of `dist/index.html` - the md5 digest the retired scripts/wdio.sh wrote into
   `.e2e-build-marker`, so a bundle rebuilt behind the binary reads as stale
   instead of failing at attach), (since `v: 3`) the `repoRoot` + `gitHead`
   of the source tree the build ran in, and (since issue #554) the optional
   `buildStartedAtMs` - the moment `buildApp` invoked `pnpm tauri build`, captured
   before the vite leg even starts. A missing stamp, a stamp whose profile /
   features / `viteE2E` / `distIndexHash` / `repoRoot` disagree with what the
   start needs, or a binary whose mtime no longer matches the stamp
   (`./scripts/dev.sh` or a plain `pnpm tauri build` relinked it) is stale. One
   stamp per profile, beside the binary it vouches for, is what lets debug and
   release coexist without thrashing rebuilds or running the wrong binary; a
   pre-release-mode `v1` stamp cannot name its profile and a `v2` stamp can name
   neither its tree nor its bundle, so both are treated as absent (one rebuild
   per bump, once). `buildStartedAtMs` is read leniently, like `gitHead`: a
   stamp written before #554 simply has none, and the mtime comparison falls
   back to the binary's own mtime, exactly as it did before.

**The mid-build edit race (issue #554), closed.** `pnpm tauri build`'s
`beforeBuildCommand` runs vite FIRST - dist/ lands within seconds - and only
then does cargo spend minutes compiling and linking. A source edit landing in
that cargo window is older than the binary's eventual mtime, so comparing
against the binary's mtime (the pre-#554 behavior) missed it entirely: the
stamp's `distIndexHash` still matched the (now-stale) `dist/` it embedded, and
`fresh: true` came back over a bundle that predated the edit. `checkBuildFreshness`
now compares the newest tracked source mtime against `buildStartedAtMs` instead
of the binary's mtime whenever a stamp carries one, which closes the window -
nothing under a TRACKED path edited after the build began can look fresh (an
untracked input is a gap in the list above, not something this comparison can
see). The workaround this used to require (manually passing `build: "force"`
after editing mid-build) is no longer necessary.

`buildStartedAtMs` is trusted only when `0 < buildStartedAtMs <= binaryMtimeMs
+ BUILD_START_SLACK_MS` in the same stamp (2000ms, absorbing legitimate
filesystem mtime granularity - sub-millisecond on ext4/APFS, up to ~1s on
HFS+, up to ~2s on FAT worst case - rather than misreading it as clock skew,
which a plain `<=` did and CI caught) - a real build does not start long after
it finished linking. Two independent clocks feed those two numbers (the
harness process clock and the filesystem), and a stamp that fails the check
falls back to `binaryMtimeMs` instead: an untrusted, future-dated
`buildStartedAtMs` would otherwise make every later edit read fresh forever,
and a filesystem whose mtimes trail the process clock would otherwise make
every source read newer than a threshold that never advances - a rebuild loop
that never converges (and, since each rebuild re-stamps with the same lagging
clock, one that also defeats `fuzz-matrix`'s warm-up-alone skip).

The Tier 2 OS helper (`e2e/agent/os/helper.ts`) keeps the older, binary-mtime-
only comparison; its build is a handful of small crates finishing in seconds,
so the mid-build window this section closes is negligible there. Left out of
scope deliberately, not missed.

mtimes alone cannot see *how* a binary was built, so before the stamp a
`dev.sh`-built binary looked fresh and `auto` attached to a hookless dist, only
to fail the post-attach `__E2E_TEST__` check - a full attach cycle plus a manual
`build: "force"` per rerun (issue #260). The stamp lives in the cargo target dir
(a build output, gitignored / outside the repo), never in the source tree. The
post-attach `__E2E_TEST__` check remains the definitive backstop.

### Cross-worktree identity (`repoRoot`, issue #488)

Binary AND stamp live in `$CARGO_TARGET_DIR`, which several git worktrees of this
repo routinely share. mtimes cannot tell one builder from another - worktree A's
sources are typically OLDER than a binary worktree B just built - so A's
freshness check passed and A drove B's app, silently testing somebody else's
code. `v: 3` closes that: the stamp records the realpath of the building tree,
`checkBuildFreshness` compares it against its own tree, and a mismatch is stale
with a reason naming both roots and the builder's HEAD:

```
Rebuilding: build stamp was produced by the checkout at /path/to/other/worktree
(@ 1a2b3c4) but this start runs from /path/to/this/worktree - one cargo target
dir serves every worktree, so that binary is not this tree's
```

`app_start` also returns `builtFrom: { repoRoot, gitHead } | null` (who built the
binary being driven, per its stamp) and, when staleness triggered the rebuild,
`rebuildReasons`. `builtFrom` is null whenever no stamp vouches for the binary
actually on disk - no stamp, an old-schema or corrupt one, or one whose recorded
binary mtime no longer matches (something relinked it) - so null reads as "cannot
say", never as "not this tree".

**The cost.** Alternating `app_start` between two worktrees that share one
`$CARGO_TARGET_DIR` now rebuilds on every switch: each tree correctly refuses the
other's binary. That is the design, not a regression - the alternative is the
silent wrong-code run. Point the trees at per-worktree `CARGO_TARGET_DIR`s if the
thrash costs more than the disk.

**The race the stamp does not close.** Nothing serializes builds across trees. A
`buildApp` in another worktree can relink the shared binary and overwrite the
stamp while a session from this tree is already attached, and the `builtFrom` that
session was handed at start is stale from that moment on. Freshness is checked
once, before attach; it is not a lease.

**Daemon-root caveat, which the stamp cannot fix.** `.mcp.json` starts the server
with no cwd pinning, so the MCP daemon builds, stamps and serves whichever tree
it was launched from. That is self-consistent - its stamp names its own root and
never looks stale - so a session driven through a daemon rooted in a DIFFERENT
worktree still runs that other tree's code, and nothing here will say so (check
the returned `builtFrom` if in doubt). When working in a worktree, drive
`e2e/agent` via `pnpm exec tsx` from that worktree rather than through a daemon
someone else started.

## Tools

Lifecycle:

| Tool | Purpose |
|---|---|
| `app_start(build?, headed?, freshProfile?)` | Build if stale, launch, attach, verify hooks. `build`: auto/skip/force. Staleness = source mtimes newer than the build-start threshold (the stamped `buildStartedAtMs` when trustworthy, else the binary's own mtime) **or** the E2E build stamp not vouching for it - wrong profile/features/`viteE2E`, relinked behind our back, or built from a different worktree (see above). Returns `builtFrom` (which tree built the binary) and `rebuildReasons` when staleness forced a rebuild. Default is headless on Xvfb even when a desktop display exists (no window pops up, no focus stealing); `headed: true` shows the window on the real display. |
| `app_stop` | Close app + driver, reap the process group. |
| `app_restart(freshProfile?=false)` | Restart keeping the profile (persisted-state repros). |
| `app_status` | Liveness: state, uptime, driverAlive, sessionDir, node. |

Observation:

| Tool | Purpose |
|---|---|
| `screenshot(name?)` | PNG as MCP image + saved to the session dir. |
| `store_state(store, path?, maxBytes?)` | Read a Zustand store; no path = key/type summary, dot path = value (JSON-capped). |
| `logs(level?, peek?)` | New ring lines since last read (error/warn/all); `peek` does not advance. |
| `wait_for(condition, ...)` | selector / hidden / app-ready / ui-idle / map-idle / active-tool / store-path. |
| `element_state(testid\|selector)` | Matching count plus visible/enabled/value/checked/ARIA state. `unique: false` flags duplicate runtime IDs. |

Interaction (each runs a cheap oracle sweep after; findings ride the result under `oracle`):

| Tool | Purpose |
|---|---|
| `click(testid\|selector\|x+y, button?, double?, modifiers?)` | Fresh element resolution, waitForClickable, or a canvas pointer at coords. |
| `type_text(text, testid?/selector?, clear?, submit?)` | Focus + type; `Control` not `Meta`. |
| `press_keys(keys[])` | WebDriver key sequence. |
| `hover(testid\|selector)` | Move the pointer over a fresh element reference. |
| `drag(from..., to..., button?, durationMs?)` | Drag between test IDs, selectors, or viewport points. |
| `scroll(testid?/selector?, deltaX?, deltaY?)` | Scroll a target into view or scroll a target/window by pixels. |
| `select_option(testid\|selector, value)` | Select a native option or a portaled Stalagmight `Select` option by value. |
| `file_dialog_answer(triggerTestid, kind, path\|paths\|cancel)` | Click the named UI control, answer the picker it opens, and verify that exact answer was consumed. |
| `store_action(store, action, args?)` | Call a Zustand action (async-aware). |
| `eval(script, args?)` | Async function body in the webview; returns its value. |
| `ipc(command, args?)` | `window.__TAURI__.core.invoke`; e.g. `get_recent_logs_since`, `check_project_integrity`. |
| `map_view(set?)` | Read or set the MapLibre view (center/zoom/bearing). |
| `map_click(lngLat, button?)` | Click a visible geographic point through the ordinary or placement path, reject half-open viewport edges, and require the map interaction sequence to advance. |
| `map_feature_click(featureType, sectionId, shotId?/stationIndex?, button?, verifySelection?)` | Resolve current WebGL geometry by semantic identity, arm a one-use overlap preference, drive the MapLibre pipeline, and prove this click was processed. Selection is also verified by default. |
| `seed(mode, name?, projectName?)` | Open a fixture / fractal fixture (`e2e/fixtures/`), or create a basic empty project. |
| `driver_script(script, timeoutMs?)` | Node-context async body with `browser`/`ipc`/`evalInApp`/`readStore`/`callAction`/`log` in scope, under the mutex, for tight loops (one MCP call per action is too slow). On timeout the helpers (and the `browser` proxy) are disabled so a still-running body cannot keep driving the app outside the mutex. |

Oracles + scenarios:

| Tool | Purpose |
|---|---|
| `check_problems` | Full oracle sweep (see below). |
| `run_scenario(name, seed?, iterations?)` | Run a named stress scenario, then a full sweep. |

## Stress scenarios

Registered in `e2e/agent/scenarios/index.ts`; each is a seeded loop that FAILS by
throwing. Small-n values for the `pnpm agent:scenarios` gate live in
`e2e/agent/scenarios-smoke.ts`.

There is no expected-failure list, and adding one back would be a regression.
`EXPECTED_RED` existed while `undo-redo-fuzz` and `undo-persistence-fuzz`
reproduced open bugs by construction; every issue-413 finding is now fixed in the
Rust tier and every classifier and tolerance was deleted with it, so both are
expected GREEN and STRICT and `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1` has nothing
left to allow. If either goes red the red is the APP's: shrink it
(`pnpm agent:fuzz-shrink`) and fix the app.

Two scenarios are EXCLUDED from the gate rather than expected to fail:
`import-dialog-fuzz` and `export-roundtrip-fuzz`, both in `SMOKE_EXCLUDED`. They
run on explicit request only (see their sections below), and both are expected
green with no classifier and no tolerance of their own. The export one is out
for two reasons of its own on top of the wall clock: an episode rasterizes the
whole survey up to three times, and its round-trip oracles create projects on
disk and import into them - neither belongs in a gate that runs every scenario
back to back on one session.

| Scenario | What it asserts |
|---|---|
| `seed-large-project` | A fractal fixture opens cleanly into a multi-section single-system view. |
| `undo-redo-churn` | Create N sections, undo N + redo N; the cheap state signature returns to baseline. |
| `undo-redo-fuzz` | Seeded random mutations interleaved with random undo/redo walks; an EXACT normalized `get_project_state` roundtrip at every history depth (see below). |
| `undo-persistence-fuzz` | Builds an undo stack covering every reachable `UndoData` variant, saves/closes/reopens it, then walks the whole RESTORED history down and back up (see below). |
| `dialog-cycling` | Opening/closing dialogs leaves no residue and no DOM growth. |
| `tool-switch-spam` | `activeTool` tracks every switch. |
| `map-storm` | A seeded `jumpTo` barrage does not stall the main thread. |
| `save-under-churn` | Mutate+save leaves `isDirty=false`, no `.tmp`, and reopens clean. |
| `import-repeatedly` | Re-importing a `.tml` keeps the undo stack at or under 100. |
| `import-dialog-fuzz` | Generates DMP/Ariane/Compass/Walls/GPX files and walks the real import UI at random; per-step coherence plus a per-flow undo-roundtrip / residue / accounting / receipt / temp-file contract (see below). |
| `export-roundtrip-fuzz` | Exports every format through the Export workspace panel and the raw store action at hostile destination paths; file/result agreement, refusal parity, a svg/png/pdf differential, byte identity, round-trip convergence, shot survival, and every way out of the workspace returning to the mode it was entered from (see below). |
| `rapid-project-cycle` | Open/close cycles leave clean project transitions. |

### undo-redo-fuzz

The oracle is a full canonical `get_project_state` snapshot per undo depth: every
field it drops is justified in `e2e/agent/lib/fuzzOracleCore.ts`'s header, map key
order is canonicalized rather than ignored, and when two canonical strings
disagree Node re-compares with a 1e-9 absolute / 1e-12 relative tolerance
(`projectSnapshot.ts`), because geometry is re-derived rather than copied after
every undo/redo. Backend truth for the model is the app's own `get_undo_stack` /
`get_redo_stack` lengths.

The snapshot is taken and compared IN THE WEBVIEW. `fuzzOracleCore.ts` holds the
serializer and the targeting digest as two self-contained functions that Node
calls directly and also injects into the page via `Function.prototype.toString()`
- one implementation, with a unit test that feeds the same state to both and
asserts byte-identical output. Each step is then a single fused `executeAsync`
(`undoRedoFuzzBridge.ts`): call the projectStore action, observe, canonicalize,
compare, return a verdict plus the digest. Full canonical strings cross the
bridge only after a comparison has already failed.

A full run today (release, HEAD) costs about 135 ms/iteration on the fractal
fixture and 217 ms/iteration on naharon, against 104 / 189 before the extra
oracles - so the undoTo/redoTo equivalence check, the per-step invariant suite,
the integrity delta and the save/reload boundary together add roughly 30% and 15%
respectively. On naharon, `integrityMs` is ~4% of step time and `canonicalMs` ~7%;
the rest is still the app. Save/close/reopen is charged outside the fused step,
at roughly a second per round trip.

That leaves the bridge at roughly 6% of a step. Measured on fractal seed 4 x60
(90 steps, `actionMs`/`observeMs`/`canonicalMs` are reported as metrics): 4775 ms
in the store action's own `flushPlanAnchors` + command + `refreshProjectState` +
`checkProjectIntegrity` chain, 810 ms in the three read-only IPC reads, 132 ms
canonicalizing, ~390 ms of WebDriver. The action chain is the boundary coverage
that found the `updateSection` and undo-fails-outright bugs, so it stays; further
speedup has to come from parallelism, which is what `agent:fuzz-matrix` is for.

The starting point is chosen and cleaned rather than inherited. The scenario
closes whatever project is open, materializes its own copy of the fixture,
DELETES that copy's persisted undo history, loads it, and then asserts both
history stacks are empty. Tracked fixtures ship with a stack
(`undo_persist_limit`, 20 rows in the fractal fixture and 2 in naharon), and a
scenario that models undo depth must not fuzz on top of somebody else's entries
or burn the headroom below the 100-entry limit. `stripUndoHistory`
(`e2e/agent/lib/fixtures.ts`) is the harness-side replica of the app's own
`Database::clear_undo_history_at`, and refuses any path inside `e2e/fixtures/`
so it can only ever touch a temp copy.

| Env var | Effect |
|---|---|
| `STALAGMIGHT_FUZZ_FIXTURE` | Which `.stalx` under `e2e/fixtures/` to fuzz, by BASENAME (`fixtureSourcePath`'s `path.basename` is the containment guard). Defaults to `DEFAULT_FIXTURE`. Recorded in the metrics and the log line so a run stays attributable. |
| `STALAGMIGHT_FUZZ_STREAM` | `v1` restores the ORIGINAL generator set, weights and value ranges, reproducing the legacy draw stream byte for byte. The default v2 stream adds the delete/split family, batches, comment moves, synthetic imports, boundary-biased values and per-run swarm weights - and necessarily draws differently, so v1-era repro lines need this variable. `v3` is v2 PLUS the import group; see below. |
| `STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P` | v3 only: share of generated input files drawn from the corruption catalog rather than written clean. Default 0.2. The same knob, with the same default, that `import-dialog-fuzz` uses. |
| `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS` | INERT. It used to keep a run going past a mismatch that was entirely a classified known bug; every classifier is deleted now that the findings are fixed, so there is nothing left to allow. Still read, and still named in the failure's `reproduce:` line, so a tolerant-looking invocation cannot be mistaken for one that relaxed something. |

Fixture choice is load-bearing, not cosmetic. The fractal fixture has no loop
closures, no zero datums and a non-null surveyor on all 32 sections; naharon has
all three plus 13 null-surveyor sections, and half the bugs below are only
reachable there.

Three oracles run, not one. The **roundtrip** comparison above; an **absolute
invariant** suite (`checkProjectInvariants`) run in-page on every observed state,
because the roundtrip only sees drift BETWEEN states and a command that corrupts
the project with an undo that faithfully restores the corruption matches at every
depth; and an **integrity delta**, which records
`check_project_integrity`'s own verdict per depth and asserts every history move
restores it. The invariant suite deliberately does not re-check what the integrity
checker owns - the split, and why, is in `fuzzOracleCore.ts`.

About 10% of history moves are taken as a MULTI-step `undoTo`/`redoTo` jump - the
history dropdown's path - verified against the same snapshots single steps
recorded. That decision is drawn from a separate PRNG (`mulberry32(seed ^
0x9e3779b9)`) so the primary draw stream every recorded repro depends on is
untouched.

Runs also cross the PERSISTENCE boundary - save to a temp path, close, reopen -
because undo history is persisted and nothing below a save point is exercised
otherwise. Save-time healing mutations and the reload's zero-trust offset
re-solve are subtracted only from comparisons that cross that boundary (an epoch
stamp per recorded depth), so within an epoch the oracle stays strict.

Each run also picks a SWARM PROFILE from the seed (merge-heavy, delete-heavy,
batch-heavy, split-heavy, move-heavy, metadata-heavy, balanced) on its own
stream, so different seeds explore different shapes of history rather than all
sampling the same uniform mix. The profile is recorded in the metrics and the log
line.

A 60-cell v2 sweep (both fixtures, seeds 1-30, x40) yielded 18 failures and a v2
repro for bugs 1, 5, 6, 7 and 8; bugs 2, 3 and 4 were not re-found and keep their
v1 lines, executable with `STALAGMIGHT_FUZZ_STREAM=v1`. The full table is in the
scenario header.

#### v3: the import stream

```bash
STALAGMIGHT_FUZZ_STREAM=v3 pnpm agent:scenario undo-redo-fuzz --seed 1 --iterations 80
STALAGMIGHT_FUZZ_STREAM=v3 pnpm agent:fuzz-matrix --seeds 1..4 --workers 1 --iterations 40
```

`import_sections` and the four `execute_*_import` commands push ONE undo entry
covering an arbitrary number of new sections, placements, station comments and
in-place edits to shots that were already there. That is the widest single
command in the app, and it is a `ProjectSnapshotPair` - the entry family findings
8 and 11 both lived in. v3 adds fifteen generators for it
(`e2e/agent/scenarios/undoRedoFuzzImportActions.ts`), about 24% of a balanced
run's draws:

- **`importSectionsSynthetic`** - fully synthetic `ImportSectionConfig` batches:
  0 to 6 sections under a 24-shot cap, all six `ImportPlacement` variants,
  duplicate names inside one batch, `reverse`, `station_comments` past the end of
  a section, and refs the targeting digest says are no longer live (the shape a
  placement takes when it names a section the run has since deleted).
- **`importSectionsNonFinite`** - one guarded shot field poisoned. This is really
  the NULL family: `JSON.stringify` spells `NaN` and `Infinity` as `null` on the
  WebDriver bridge exactly as it does on the real `invoke`, which is the whole
  premise of `src/utils/importPayloadGuard.ts`.
- **`importSectionsWithOverlap`** - `existing_shot_updates`, the DMP wizard's
  overlap interpolation, aimed at the doubly-anchored range the wizard's
  `useMemo` greys out. The backend refuses it independently
  (`command_import.rs` collects zero datums, datum-backed merges and aligned
  closures into `anchor_indices` and skips the section when one sits on each
  side), and that parity is only testable if something sends the payload the UI
  will not.
- **The four file importers** plus the parse-only `importDmp`, on files built by
  `surveyFileSpecs.ts` / `surveyFileCorrupt.ts` - the same writers
  `import-dialog-fuzz` uses, never a fixture.
- **The cheap probes** - `calculateFileHash`, `checkDuplicateImport`,
  `checkSectionHashes`, `suggestImportPlacement`, `saveAfterImport`.
- **The motif pair** - `createMotifSection` plants a section whose shots are a
  motif derived from a small integer key, and `probeSuggestMotif` asks
  `suggest_import_placement` for that same motif jittered well inside the
  suggester's cutoffs. Both are pure, so the shrinker can re-derive either half;
  until they landed, the probe had only ever exercised the empty return.

**v1 and v2 draw identically.** The group is filtered out before the weighted
name array is built, and appended at its END when it is not, so every index
`ctx.pick` reads still holds what it held. The `import-heavy` swarm profile is
likewise appended only under the v3 gate, because merging it into the profile
list would have changed the divisor every v2 seed picks its profile with. Both
properties are pinned by `e2e/agent/lib/undoRedoFuzzImportActions.test.ts`, and
both were re-measured against the pre-v3 tree: v1 fractal seed 4 x60 and v2
fractal seeds 1 x40 / 4 x60 are metric-for-metric identical before and after.

A **fourth oracle** applies to v3 payloads only. `mustBeRejected` names the two
shapes that cannot be deserialized at all - a required `f64` arriving `null`, and
`date: ""` (issue #478, here on the route that BYPASSES the wizard's
normalisation) - and the run FAILS if the app accepts one. It says nothing about
the OPTIONAL shot numbers, where `null` legally means "absent": the frontend
guard refuses those because dropping a measurement is data loss, but the backend
accepting them is not a defect. It is a pure function of the recorded args rather
than a flag on the drawn action, which is what lets a replayed trace assert
exactly what the recording did.

A v3 run legitimately produces `log-error` findings from the oracle sweep: a
malformed file the parser refuses, or the deliberate missing-file probe, is the
app reporting a problem properly. Those are findings at severity `error`, which
does not fail a run.

The introducing sweep was 8 cells (both fixtures, seeds 1-4, x40), 14 more on the
seven seeds whose swarm profile is `import-heavy` (14, 15, 21, 23, 26, 56, 59,
both fixtures, x60), and two malformed-heavy runs at
`STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P=0.8`. All green. The one red it produced was
the ORACLE's - a claim that the backend refuses a non-finite
`existing_shot_updates` field, which it cannot, because `JSON.stringify` spells
NaN `null` and `Option<f64>` reads `null` as "leave this field alone". That
measurement is written up in the scenario header as an observation, not a
finding: the backend check is a Rust-API defence and cannot be cited as
protecting the frontend.

##### File inputs: what a trace carries, and what it rebuilds

A file-backed import takes a PATH, and a path cannot be recorded verbatim - the
recording machine's absolute path means nothing to a replayer, and a trace is
meant to be attachable to a bug report. So a file-backed move records **two**
steps:

1. a `genFile` carrying the generator SPEC and a session-relative `rel`
   (`fuzz-files/<content-hash>.<ext>`), and
2. the `mutate` whose `args[0]` is that same `rel`.

The replayer rebuilds the file from the spec into its OWN directory and then
resolves `args[0]` against it. `surveyFileWriters.ts` is a pure function of the
spec, so the rebuilt bytes are identical; the trace carries no fixture (which the
repo forbids), no bytes, and no absolute path.

`FILE_ARG_ACTIONS` (`e2e/agent/lib/fuzzTrace.ts`) is the single list that says
"argument zero is a path", and `readTrace` refuses a trace that points one of
those actions outside the session directory - the same rule every other v2 `rel`
field has, and for the same reason: a trace is untrusted input, and these
arguments reach the filesystem instead of the model.

Generated files live under the run's own temp directory and are removed in the
same `finally` as the save/reload copies. Nothing is lost by deleting them (the
trace regenerates them), and `pnpm agent:fuzz-shrink` replays this scenario
hundreds of times per shrink, so leaving them behind is not an option.

Verified end to end: v3 fractal seed 1 x80 recorded with `STALAGMIGHT_FUZZ_TRACE`
and replayed with `--trace` reproduces the run metric for metric (34 mutations,
31 undos, 19 redos, 18 rejected, 2 no-ops, depth 15, 66037-byte snapshot) with
all five `genFile` steps re-materialized.

##### What v3 does not reach

`suggest_orphan_placements` is not a `projectStore` action - `App.tsx` and the
three import dialogs invoke it directly - and this scenario dispatches store
actions by name. Widening the bridge to dispatch IPC commands by name for one
read-only probe would give a recorded trace a much larger surface; the probe is
covered by `import-dialog-fuzz`, which drives the dialogs that call it.

There is no Rust-tier analogue of the import group, and adding one is a design
problem rather than a transcription. `Refs::learn`
(`src-tauri/crates/core/src/core/state/fuzz_undo_tests.rs`) pairs what a
recorded move created against what the replayed move created POSITIONALLY, with
`zip` and no length check - sound only because every existing `Op` creates a
fixed number of entities (at most one section, one shot, one closure; `BatchOp`
has no creating variant at all). An import creates an arbitrary number in one
move, and `execute_import_sections` drops sections whose placement does not
resolve, so a replay that places one fewer would pair a PREFIX, silently, and
the shrinker would minimize toward the mis-pairing instead of toward the bug.
Whoever adds it has to give `learn` a checked, arity-aware pairing first.

The findings are tracked at
<https://github.com/Stalagmight/Stalagmight/issues/413>, one comment per finding
with its mechanism, repro and fix direction; the scenario header is the
authoritative copy of the repro commands.

Every one of those findings is FIXED in the Rust tier, so every known-bug
classifier and every tolerance here has been deleted per the delete-when-fixed
contract: a mismatch of any shape, at any depth, fails the run again. The repro
lines are kept verbatim as regression checks, and they are GREEN.

The last one to close was found by this tier after the earlier waves: **finding
11's residue**. That fix re-applied the save heals to a restored project as a
CARRY-FORWARD from the live project, so a `ProjectSnapshotPair` recorded before a
save that RE-CREATES a section (a batch containing a create, a range-split, ...)
still came back without the gauge anchors the save captured - there was no live
section to carry them from, and roughly half the seeds on either fixture reached
it. `pnpm agent:fuzz-shrink` cut fractal seed 1 x40 to two steps for it:
`executeBatch(...+crtSec+...)` then `save`. Closed in b9d6b14b by making snapshot
pairs self-refreshing walls - an undo re-records the entry's `after` half from
the live project before restoring `before`, and a redo mirrors it, so pair
undo/redo are exact inverses by construction. No classifier was ever added for
it; the scenario stayed red until the app was fixed.

### undo-persistence-fuzz

The sibling scenario, and the one that took bug 8 seriously as a MECHANISM rather
than as a single defect. Bug 8 is "shots are renumbered on load, persisted undo
entries are opaque JSON that nothing remaps, and the in-session allocator draws
from the same id space, so a stale id collides". None of that is specific to
`CreatedShotSnapshot` - so this scenario goes looking for the siblings, on
purpose rather than by luck.

Every iteration is one complete, independent cycle:

1. open a fresh copy of the fixture with its persisted undo history stripped;
2. run a fixed COVERAGE PLAN (`undoPersistenceCoverage.ts`) whose targets and
   values are seeded but whose SET and ORDER are not, so the stack carries every
   `UndoData` variant a live command can produce;
3. assert the shot ids really have diverged from positional order (the collision
   precondition), rather than assuming the plan produced it;
4. undo a few entries so the REDO stack is non-empty at save time;
5. save, close, reopen - both stack lengths must come back exactly; and
6. unwind through every restored undo entry to depth 0, then redo back to the
   top, **through the restored redo entries**, which nothing else in the harness
   had ever exercised.

The plan's order is the load-bearing part, and it is not the obvious one.
`Shot::id` is serialized inside a project snapshot, so `UndoData::ProjectSnapshotPair`
restores the PRE-SAVE id space wholesale and heals every stale id beyond it: a
snapshot-pair entry is a WALL, and the exposed window is exactly "from the depth
the reload lands on, down to the first pair beneath it". The first arrangement put
all the pairs above the id-bearing entries and 60 iterations found nothing new;
inverted - pairs as low as their preconditions allow, every id-bearing entry
contiguous at the top - two defects fell out on the third iteration.

Beyond the main fuzzer's three oracles it adds two of its own: COVERAGE (every
variant a guaranteed plan step promises must be present on the RESTORED stack)
and STACK SURVIVAL (both lengths must equal what they were at save time). Six
`UndoData` variants are reported once per run as out of scope: `MergeInfo`,
`UnmergeInfo`, `StationMoveInfo`, `ShotSnapshot`, `SectionSnapshot` and
`ProjectSnapshot` still have restore arms in `undo()` but have no live producer
left, so a project whose history was stripped can never construct one.

| Env var | Effect |
|---|---|
| `STALAGMIGHT_FUZZ_FIXTURE` | Same as the main fuzzer: which `.stalx` to build on, by basename. |
| `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS` | INERT here too - bug 1, bug 8 and findings 11/12/13 are fixed and their classifiers deleted, so there is nothing to walk past. |
| `STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET` | `1` moves the plan's mid-section delete onto the section the shot entries target. That was finding 13's repro, and it used to be terminal: the stale id ran past the renumbered range, the undo failed, and the unwind could not continue. The wedge is fixed, so the flag now walks the whole restored stack (measured: 3 cycles x 19 entries); it stays off by default because the two arrangements are different id-space shapes and the plan's coverage promises are written against the default. |

It found two new bugs, both written up on issue #413 and both now FIXED
(b9c9d649), their `open_undo_bugs_tests.rs` tests un-ignored. **Finding 12**:
after a save/reload, undo or redo of `SetShotKind` / `UpdateShot` edited the
WRONG shot - `UndoData::InverseCommand` and `UndoData::PreviousValues { shot_id }`
both named a shot by its in-session id, and `redo_inner`'s catch-all re-executed
the original command with the same id. It needed no delete, unlike bug 8; one
insert-before was enough, and nothing about the resulting project looked wrong.
**Finding 13**: when the section HAS had a shot deleted, the allocator has run
past the renumbered range, so the stale id missed instead of colliding - the undo
failed outright, `undo_inner` pushed the entry back, and the history was WEDGED
with every entry beneath it unreachable.

Both were the same root cause as bug 8 and got the same fix: undo entries address
a shot by `uuid` (the route `StationKey` and `CreatedShotSnapshot { uuid }`
already took), with a load-time pass that DROPS an entry whose target no longer
resolves rather than leaving it to wedge the stack.

Found here on naharon and since FIXED (b9d6b14b, f16c96a8): **finding 27** - an
undo of `update_shot` DROPPED `section_offsets.0` where the state it restores had
`[0,0]`. In-session (no save involved), and the exact mirror of finding 2:
a67dd1b5 made adjust-style offset writes drop a key that lands within
`OFFSET_DUST_M` of zero, and an undo re-executes the command through that same
cascade, so a key whose legitimate value is (0,0) was deleted rather than left
alone. Fixed on the rule that an inverse RESTORES rather than RE-DERIVES (the
undo data captures the rigid set's offsets pre-command and puts them back
verbatim), after which f16c96a8 made a dust-zero entry unrepresentable outright -
one normalizing writer, plus re-normalization at the loader and at snapshot
restore, the two boundaries that ingest maps this app did not write.
`STALAGMIGHT_FUZZ_FIXTURE=naharon.stalx --seed 5 --iterations 2` is kept as its
regression check; the shrinker had reduced it to five steps.

### import-dialog-fuzz

The first scenario that drives the app by CLICKING it rather than by calling
store actions, so it is the first one whose oracle has to know what is on screen.
That job belongs to `e2e/agent/lib/importScreenDigest.ts`: one self-contained
function, injected exactly the way `fuzzOracleCore.ts` injects its oracle, that
returns in a single in-page pass which screen is up, which testids are actionable
and what the project looks like. `scenarios/importDialogModel.ts` owns the screen
table, the verified testid inventory and the escape paths;
`scenarios/importDialogGenerators.ts` is the weighted move catalog;
`scenarios/importDialogBridge.ts` holds the testid ALLOWLISTS, which are the
security boundary for a replayed trace exactly as `REPLAYABLE_MUTATIONS` is for
store actions.

An EPISODE opens a fresh fixture and runs a few FLOWS with no reload between
them; a FLOW is one input file from an entry point to whatever ends it.
Per-step oracles cover the screen table, the error boundary, the project
invariants, unexpected error dialogs and wedges, plus an integrity check after
any step that pushed an undo entry. Per-flow oracles are in
`e2e/agent/lib/importFlowOracles.ts`: the undo roundtrip, dialog/placement/context
residue, the receipt's numbers against the project's delta, the receipt's own
rendering, and leftover staging files.

It covers all five importable families - MNemo `.dmp`, Ariane `.tml`/`.tmlu`,
Compass `.dat`/`.mak`, Walls `.srv`/`.wpj` and GPX - which is not just five
parsers but two flow SHAPES: `.dmp` is the only additive import, and every other
extension routes to `new-project-for-import` whether or not a project is open
(with one open it goes through `import-as-new-project`, whose confirm closes the
project first). So a non-DMP flow always creates a project in the middle of
itself. `STALAGMIGHT_FUZZ_IMPORT_FORMAT` is a csv of family names that restricts
the draw, and it is the natural third matrix axis.

Input files come from three sources: a seeded spec rendered by
`e2e/agent/lib/surveyFileWriters.ts` / `surveyFileWritersText.ts` (valid-biased,
or run through the named corruption catalog), a tracked `.dmp` fixture, or - when
`STALAGMIGHT_FUZZ_CORPUS_DIR` points at one - an entry from a PRIVATE corpus of
real survey files. Generated files reach the parsers' edges; real files reach the
shapes nobody would think to generate. A run with no corpus counts
`corpus:absent` and is exactly as valid, just narrower.

**Nothing a corpus run writes ever names a corpus file.** An entry is identified
everywhere by `corpusFileId`, a 5-hex fold of its relative path
(`e2e/agent/lib/fixtureCorpus.ts`), because a survey filename can name a cave, a
landowner or an unreleased project - and a metric key, a log line, a failure
payload and a recorded trace are all things that end up attached to an issue.

A trace is the artifact that rule is easiest to lose, so it is enforced rather
than observed. A `corpusFile` step carries `corpusId` plus `corpusHash`, and
nothing that could be a name; `TraceRecorder.record` is the redaction boundary,
so a caller that hands over a path gets it folded before the step is stored; and
`writeTrace` refuses outright to emit a corpus step named any other way, whoever
assembled it. The corpus ROOT is not recorded either -
`STALAGMIGHT_FUZZ_CORPUS_DIR` is an absolute path on the recorder's disk, so it
is excluded from the trace's `env` header and never restored on replay.

**`corpusId` labels; `corpusHash` resolves.** The 5-hex crc20 is 20 bits, which
is ample for telling two files apart in one run's output and far too little to
look one up: discovery permits 20000 entries, and by the birthday bound a corpus
of a few thousand files probably holds a colliding pair. Taking the first match
would materialize the wrong private file and then open it under the recorded
file's name. So `corpusHash` (16 hex of SHA-256 over the same relative path,
`fixtureCorpus.corpusPathHash`) is what replay resolves by, and it is a lookup
field only - nothing renders it, so `corpusId` remains the sole identifier in
every log line, metric key, failure payload and `describeSteps` line. A bare-id
lookup that matches twice resolves to NOTHING and counts `corpus:ambiguousId`;
`discoverCorpus` reports the colliding ids (never the paths) and `describeCorpus`
warns about them on every run that has them. A reference this machine does not
have is a skipped and counted move, never a failure.

**A corpus entry materializes as its whole directory TREE.** A Compass `.mak`
names `.dat` files by relative path and a Walls `.wpj` names its surveys with
`.PATH` directives that DESCEND into subfolders, so the copy is recursive and
structure-preserving, bounded at 64 MB / 4000 files / 16 levels. Over any bound
it copies nothing and says which one, because a half-copied project is
indistinguishable from a corrupt one from inside the app and the fuzzer would
file the difference against the importer.

**The discovery walk goes 16 levels too, and counts what it refuses.** The two
depths are the same number on purpose and a test pins them equal: a walk
shallower than the copy hides files the copy would have taken, a walk deeper
draws entries the copy then refuses every time. It was 8, and the shallow half
is what that cost - on the corpus this was written against, 8 levels found 352
importable files and 16 finds 396. The 44 in between are 41 more `.srv` surveys
and three more Walls project files, one of them 12 `.PATH` directives over a
14-file tree. Nothing reported the gap, because a file that is never discovered
leaves no trace anywhere: so directories past the bound are now counted,
`describeCorpus` prints that count and calls its own file total a lower bound,
and `import-dialog-fuzz` sets `corpusDirsTooDeep`. A corpus that outgrows 16
levels is then a number rather than an absence. The walk is also capped at 20000
visited entries and does not follow symlinks - which is what makes it terminate
on a directory that links back into itself.

Before it fuzzes anything, the run pushes a valid-biased spec of EVERY enabled
format through the real parse command and checks the count against
`selfCheckClaim`. That is the writer/parser drift detector: without it a writer
that had drifted would have every file rejected at the door and the whole run
would pass green having exercised nothing.

Not in `pnpm agent:scenarios`. It is listed in that file's `SMOKE_EXCLUDED` and
runs on explicit request only - `pnpm agent:scenario`, `pnpm agent:fuzz-matrix`,
or the MCP `run_scenario` tool. That is not an expected-failure list in disguise:
the scenario has no classifier and no tolerance, so a red from it is the app's.

```bash
pnpm agent:scenario import-dialog-fuzz --seed 7 --iterations 8
pnpm agent:fuzz-matrix --scenario import-dialog-fuzz --seeds 1..4 --workers 2
# one format family per cell:
pnpm agent:fuzz-matrix --scenario import-dialog-fuzz \
  --axis STALAGMIGHT_FUZZ_IMPORT_FORMAT=dmp,tml,compass,walls,gpx --seeds 1..3 --workers 2
# sweep a knob as a third matrix dimension:
pnpm agent:fuzz-matrix --scenario import-dialog-fuzz \
  --axis STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P=0,0.5,1
# widen the inputs with a private corpus (never baked in, never named in output):
STALAGMIGHT_FUZZ_CORPUS_DIR=~/data/my-survey-files \
  pnpm agent:scenario import-dialog-fuzz --seed 4 --iterations 6
```

Its knobs, all read at call time so a replayed trace's restored environment
reaches them: `STALAGMIGHT_FUZZ_IMPORT_FORMAT` (all five families),
`STALAGMIGHT_FUZZ_IMPORT_FLOWS_PER_EPISODE` (3), `_MAX_FLOW_STEPS` (140),
`_MALFORMED_P` (0.2), `_INTERJECT_P` (0.08), `_AUX_INTERJECT_P` (0.04),
`_SETTLE_MS` (12000), `STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P` (0.3),
`_GPS_PASSABLE_P` (0.6), `_SUGGESTIVE_P` (0.5), `_FORCE_STATIONS` (on), plus
`STALAGMIGHT_FUZZ_CORPUS_DIR` (unset) and `STALAGMIGHT_FUZZ_CORPUS_DRAW_P`
(0.25).

Two interjection streams, not one: `_INTERJECT_P` drives the original
Escape/route/undo/redo/save set, and `_AUX_INTERJECT_P` drives the wave-3
additions (Ctrl+E, an app-mode switch, `closeProject`, and Ctrl+O with an armed
picker). They are separate PRNG streams so that adding the second left the
first's draws bit-identical, which bounds how far the existing repro lines
moved - the convention `mulberry32`'s header states.

Ctrl+E no longer opens a modal: at idle it toggles the Export workspace, which
the screen model classifies as `idle` (it is an app mode, not a screen), so the
flow starters keep running inside it and exercise the read-only / ImportBlocked
path. The app-mode interjection clicks `mode-survey` / `mode-plan` (and
`mode-data` where the carousel renders it), and inside Export may leave through
the toolbar's `export-close-button`; it never clicks `mode-export`. The EXPORT
CHORD oracle asserts that a Ctrl+E pressed behind any gate the keydown handler
consults - an open dialog, an independent overlay (`screenOccupants`), a map
placement, the fullscreen data table, or no project open - is INERT: `appMode`
does not move and no Export panel appears. The gates and the mode are read by
the bridge on the dispatch tick, not from the older pre-move digest; the one
excuse is a project switch landing inside the settle window, which forces
Survey on its own. Ungated chords are counted by effect (entered / left /
none), never asserted. The judgement is the pure `exportChordVerdict` in
`importDialogModel.ts`, unit-tested in `importDialogActions.test.ts`.

The open findings it has produced are written up in the scenario file's header,
each with the `reproduce:` line that still runs.

### export-roundtrip-fuzz

The mirror image of `import-dialog-fuzz`: it drives survey data OUT of the app,
and then back in. Each iteration draws a `{format, route, path shape, options}`
and exports through the real surface, then asserts that the app's report, the
filesystem and what the user was told all agree.

Two routes, because they are not interchangeable. The Export workspace panel
(`ExportControls` inside `ExportPanel`, over `exportStore`) pushes the answered
path through `ensureExtension`, so a destination the user typed is REWRITTEN
before the command sees it. The raw store action hands the path over verbatim,
which is the only way the backend's own guards are reached. Each route is
asserted against its own contract rather than forced to agree move for move.
There is no modal route: the Export dialog is retired, and a recorded trace
whose `export` step names the `modal` route is refused on replay with a message
saying so (re-record it).

Every panel move is a round trip through the workspace. It draws the mode it
starts in (`survey`, `data` or `plan`), how it enters (the hamburger row
`toolbar-export-button` behind `app-menu-trigger`, Ctrl+E, or `enterExport`)
and how it leaves (the toolbar's `export-close-button`, the panel header's
`export-panel-close`, the same menu row (labelled "Close export" there),
Ctrl+E, or Escape, which may spend up to three presses clearing transient
state first). The path is recorded in the trace step and replayed as-is; a
panel step recorded before this carries none and replays as `enterExport` /
`closeExport` from Survey. A drawn control that is absent falls back to the
store action and is counted, never asserted. After the exit, X10 ("Close
export returns to the recorded mode", `checkExportCloseReturn`) asserts that
`appMode` is back to the mode before entry and that `exportReturnMode` is
null. X8 (modal vs panel option independence) retired with the modal; X8b, the
option reset on a project switch, remains.

Ten destination-path shapes, each aimed at a rule somewhere in the stack:
`NAME.TMLU` (the exporter lowercases before comparing), `a.tml.txt` (the
extension is replaced, not appended), no extension, a trailing dot, a name
carrying U+202E, a ~200-character name, `%`/`"`/space, and the two that name a
file the open project owns - its `.stalx` and the `.plnx` dive-plan sibling.

| Env var | Effect |
|---|---|
| `STALAGMIGHT_FUZZ_EXPORT_FORMATS` | csv list of `csv,json,tml,tmlu,png,svg,pdf,preview,diveProfile`. Default all nine. An unknown name is a hard error, not a silent drop - a typo in a matrix cell would otherwise run the full set and be reported as covering something it did not. |
| `STALAGMIGHT_FUZZ_EXPORT_MAX_DPI` | Ceiling for the DPI draw, default 600. There is no backend ceiling (`export_png` bounds the derived PIXEL dimensions at 16384, not the DPI), so drawing freely mostly measures the size bail. Raise it to go looking for that deliberately. |
| `STALAGMIGHT_FUZZ_EXPORT_ROUNDTRIP` | `0` turns the X5/X6 import chains off. They are the expensive half: each creates projects on disk and imports into them. |
| `STALAGMIGHT_FUZZ_EXPORT_MOVES` | Export moves per episode, default 3. |
| `STALAGMIGHT_FUZZ_EXPORT_SETTLE_MS` | In-page settle cap, default 20000. |

```bash
pnpm agent:scenario export-roundtrip-fuzz --seed 1 --iterations 10
# round-trip heavy: every draw is a .tml or .tmlu
STALAGMIGHT_FUZZ_EXPORT_FORMATS=tml,tmlu \
  pnpm agent:scenario export-roundtrip-fuzz --seed 4 --iterations 20
pnpm agent:fuzz-matrix --scenario export-roundtrip-fuzz --seeds 1..3 --workers 1 --iterations 8
# one format per cell:
pnpm agent:fuzz-matrix --scenario export-roundtrip-fuzz \
  --axis STALAGMIGHT_FUZZ_EXPORT_FORMATS=csv,json,tml,png,svg,pdf --seeds 1..2
```

The oracles (X1-X10, X8 retired), the two probe results it recorded rather than asserted (the
`.plnx` x image guard asymmetry, and the inert `ExportSettings` config
defaults), and the findings it produced are all written up in the scenario
file's header, each with the `reproduce:` line that still runs.

**A recorded run replays as its whole STRUCTURE.** Per episode the trace carries
a `reload` step, one `export` step per move, and one `oracle` step per
structural oracle the rotation selected - each carrying the inputs that oracle
DREW. A replay walks that list through the same dispatch table the recording
used, so it runs the same oracles at the same points on the same inputs, and it
consumes no randomness at all. Both halves of that are load-bearing: with
`reload` ignored, a multi-episode replay ran episode 2 against whatever episode
1 left behind; with the inputs re-drawn, X3's option set and X4's container
extension came off an rng the replay had advanced differently, so an
option-dependent failure was retested with options that do not provoke it.
Measured on seed 5 x 3 episodes: before, 11 of 18 oracle metrics differed
between the recording and its replay (X3 ran 2x then 1x, X4 lost its `.tmlu`
arm, X7 ran 1x then 2x on a project that had already been mutated); after, all
18 agree and the replay reports the same 3 episodes and 6 export moves.

Its first run against a fixture other than the default found that a real
532-shot survey did not round-trip through `.tmlu` (finding 4 in that header):
78 shot uuids were re-minted, because the exporter dropped the UUID on the
reversed pre-zero-datum leg, and every measured-zero LRUD came back null. The
first half is fixed; the second is Ariane's absent sentinel and is now written
up in both lossiness tables, with the oracle folding that one documented
equivalence and nothing else.

Neither was the whole story. Finding 5 classifies the 198 residual keys into
three families: 150 were one physical leg seen through the exporter's deliberate
reversal (X6's key is now direction-agnostic, which is a fact about the key),
2 were this harness failing to see station comments at all, and 48 are the
exporter re-hanging branch depths to fit the format - which is designed,
tested behaviour and needs a product call rather than an oracle change.

All three are now closed - the depth family turned out to be the exporter
re-hanging a branch's datum while preserving every measurement (matched by uuid:
of 24 shots, length changed on 0, azimuth on 0, LRUD on 0, descent on 1, and
that one shot's source descent exceeds its own tape), so X6 keys the descent
rather than the absolute depths and counts the re-hangs as a metric.

Closing them let X5 run on that fixture for the first time, and finding 6 is its
own correction: the chain settles at the THIRD export there, not the second,
because the first import inherits its section order from the source project
rather than deriving it. T3 and T4 are byte-identical, no boundary moves and no
shot changes section - so X5 now asserts a fixed POINT rather than a fixed index
and reports `x5:settledAt:N`.

Both fixtures are green.

### Three tiers, and which one gates CI

The harness is the middle tier of three, and it is worth being clear about what
each one buys:

| tier | drives | catches | runs |
|---|---|---|---|
| `src-tauri/crates/core/src/core/state/fuzz_undo_tests.rs` | `AppState` directly | undo/redo model + persistence regressions | **every CI run** |
| this harness | the real app over IPC | anything the frontend store or the IPC boundary breaks | on demand |
| `open_undo_bugs_tests.rs` | `AppState` directly | one reduction per finding, kept as its regression guard | **every CI run** |

`open_undo_bugs_tests.rs` holds nothing `#[ignore]`d today: every bug it pinned is
fixed and every test was un-ignored as the fix landed, which is what the module's
own rule asks for. Its filters are still the way to run the whole family together,
since two findings are pinned by a SEED in the fuzz module rather than by a
reduction in the red one:

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib -- open_undo_bugs finding_14 bug_7
```

The Rust tier exists because the harness GATES nothing: it needs a built desktop
app and a display. It runs the same model - a canonical snapshot per undo depth, asserted
after every move - about a thousand times cheaper per sequence, which is what
makes it affordable as a `#[test]`. See docs/testing-recipes.md for how to run it
and its soak.

It is not a replacement. It cannot see the projectStore actions or the IPC
serialization, which is where the `updateSection` double-option bug (finding 5)
was found and where the undo-fails-outright bug (finding 6) surfaced. Equally it
sees two things the harness cannot: `station_offsets` / `station_comments`, which
`ProjectIpc` omits entirely, and the app's own `#[cfg(test)]`
`debug_assert_merge_alignment`. Findings 14, 16 and 17 came from those.

**Every finding the harness found is now reachable in the Rust tier too, and
classified by id there.** Its header carries a BUG IDENTIFICATION MATRIX: per
finding, the mode, seed and move count under which it fires and is named. That
is worth having for two reasons beyond convenience - a finding you can only
reach by driving a desktop app is a finding nobody re-checks after a refactor,
and the matrix is what made three duplicates and three NEW defects visible
(finding 15 is finding 3 re-measured; findings 16 and 17 and finding 14's
mirror image were found while building the modes). The harness stays the tier
that owns everything above `AppState`.

### Record, replay, shrink

Triage used to be the expensive human step: a failure gave you a seed and an
iteration count, and finding out which of the 37 mutations mattered meant seed
sweeps and bisecting by eye. Both fuzzers now record a TRACE, replay one, and
minimize one automatically.

**Record.** Every fuzz run collects its RESOLVED move list - each generator draw
already collapsed into a concrete `projectStore` action and its arguments - and
writes it to `<session>/traces/<scenario>-seed<N>-<stamp>.json` whenever the run
FAILS. The path is appended to the failure message. `STALAGMIGHT_FUZZ_TRACE=path`
also writes the trace of a run that PASSED, which is how you capture a baseline
to shrink against later. Recording is observe-only - it draws nothing, reads no
app state and runs after the decision it records - so a seed's trajectory is
identical with it on or off (re-verified against the bug 1 and bug 8 repro
lines).

The header carries scenario, seed, iterations, fixture, generator stream, the
`STALAGMIGHT_*` knobs that were set, the git rev and the failure message with its
parsed signature. `undo-persistence-fuzz` resets its recorder per iteration,
because its cycles are independent: the trace is of the cycle that failed.

**Replay.**

```
pnpm agent:scenario undo-redo-fuzz --trace <file> [--release]
```

Replay is a MODE of the scenario, never a separate one. Only the source of the
decisions changes: the fused step, the three oracles, the model bookkeeping, the
classification and the failure payload are the same code, so a replayed failure
is the same failure rather than a lookalike. A replayed run says so in its
`reproduce:` line.

The recording's ENVIRONMENT is part of the repro, so `--trace` restores the
header's `STALAGMIGHT_*` knobs (fixture, tolerance, stream, the persistence
ones) and prints what it restored. It has to happen before `./scenarios` is
imported - the fuzz scenarios freeze those knobs into module-level constants at
import time - which is why both `agent:scenario` and `agent:fuzz-shrink` import
the registry dynamically. A trace recorded by a different scenario, or on a
different fixture, is REFUSED: the steps carry resolved section and shot ids, so
replaying them against another project drives moves at ids that do not exist
there and reports an invented failure (or none) while the operator believes they
are looking at the recorded one.

Two rules make replay work on a trace that has been CUT, which is what the
shrinker needs:

- a history move whose precondition no longer holds (undo at depth 0, redo at
  the top) is SKIPPED, and a jump wider than the reach is CLAMPED to it. Skipping
  asserts nothing, so it cannot turn a failing candidate green by hiding
  something; it just declines to pretend a move was possible. Every skip is
  counted.
- the closing unwind/rewind is never recorded and always REGENERATED. A shrink
  therefore cannot remove the walk that visits every depth in both directions,
  which is where most failures are actually observed.

`undo-persistence-fuzz` skips its PLAN's own preconditions on a replay - id
divergence, the renumbering check, variant coverage - because a shrunk trace is
by construction not the plan. No app oracle is skipped on either scenario.

**Shrink.**

```
pnpm agent:fuzz-shrink <trace.json> [--release] [--max-tests N] [--out FILE]
```

Textbook ddmin (`e2e/agent/lib/fuzzShrink.ts`) over the recorded move list, with
a candidate cache. It replays the whole trace first to establish the baseline
signature, then repeatedly replays subsequences until no single step can be
removed, and writes `<trace>.min.json` plus a numbered action list.

Exit status is the signal, so a recorded trace doubles as a regression check:
**0** only when the trace reproduced and was minimized, **1** when it replayed
GREEN (the bug is fixed, or the failure needed something the trace does not
carry) or the shrink itself threw.

A candidate "preserves" the failure only when its FailureSignature matches:
headline, first differing path, the `error:` line of a move that failed outright,
and every classifier id the message named. Digits and array indices are
normalized out of those, because removing a mutation moves every depth and index
while the defect stays the same; the sentence shape, the field at the end of the
path and the classifier ids are NOT normalized, so a candidate that fails
DIFFERENTLY is rejected exactly like one that passes. That is standard ddmin
practice and it is what stops a shrink wandering onto a different failure than
the one the trace recorded; the count of rejected-as-divergent candidates is
reported.

Measured, on the two traces that were red when the shrinker was built (both bugs
are now fixed):

| trace | before | after | replays | divergent, rejected | per replay | wall clock |
|---|---|---|---|---|---|---|
| bug 1 (`undo-redo-fuzz`, fractal seed 1 x40) | 43 | **2** | 28 (+20 cached) | 3 | 1.8 s | 52.5 s |
| finding 12 (`undo-persistence-fuzz`, seed 1 x1, strict) | 20 | **4** | 58 (+41 cached) | 20 | 1.7 s | 97.6 s |

Both results are 1-minimal, and both replay deterministically twice. The finding
12 one is the better demonstration: its minimal list is

```
1. createSection(shots=3)
2. createShot(s=32,before=3200000)
3. setShotKind(s=32,shot=3200003,Jump)
4. save
```

which is, step for step, the synthetic Rust test that was written for that
finding by hand - create a section, insert before the first shot so the ids stop
matching positional order, mark a shot whose id will land on a different slot,
save. The shrinker got there from a 20-step cycle without being told what the bug
was. Its 20 divergent rejections are the guard doing its job: subsets that still
failed, but as bug 8 or as the stale-id wedge rather than as finding 12.

The whole shrink runs against ONE app session -
each scenario run already closes the project, materializes a fresh fixture copy,
strips its history and asserts both stacks came up empty, so a replay inherits
nothing from the one before it.

**From a shrunk trace to a red Rust test.** The action list is already the test
body: each `mutate` line is one `Command` with its arguments, in order, and the
`save` line is `state.save_project(Some(path))` followed by a reopen if the trace
crossed the boundary. Add the closing `undo()` / `redo()` walk the replayer
regenerates, assert what the fuzzer's diff said should have been restored, and
`#[ignore]` it per the convention in
`src-tauri/crates/core/src/core/state/open_undo_bugs_tests.rs`. Everything is synthetic by
construction - a trace holds generated values, never fixture data.

**One caveat, worth knowing before you trust a minimal trace.** ddmin finds the
smallest input with the same SIGNATURE, and two defects that produce the same
message are indistinguishable to it. The bug 1 shrink above lands on
`executeBatch(...)` + `save`, whose failure is really finding 11 (a pre-save
snapshot pair reverting the save's gauge capture) - `undo-redo-fuzz` has no
classifier that separates the two, so both read as
`create-section-redo-drops-gauge-anchors`. The shrinker did exactly what it
promises; the ambiguity is in the message. Read the minimal action list before
writing the test, and if it does not look like the bug you started from, that
ambiguity is the reason. (`undo-persistence-fuzz` names the entry each move
consumed, which is what resolves it there.)

## Oracles

- **Cheap sweep** (auto after every mutating tool): crash-marker glob (fatal
  even if the process survived), React error-boundary probe, new error/warn ring
  lines since an internal cursor. A `dropped`/`ring-overflow` warning means the
  500-entry ring rotated past the cursor.
- **Full sweep** (`check_problems`): cheap sweep plus `check_project_integrity`
  (`save_blocked` = fatal, non-empty messages = error), leftover write-staging
  files (`*.stalx.tmp`, `*.download.tmp`, `*.vacuum.tmp`) beside
  `projectStore.projectPath`, `get_memory_stats` vs a session baseline
  (total RSS > baseline*1.5 + 300MB = warning, with per-process breakdown), and
  a read-only responsiveness probe (a hung IPC = possible command-lock deadlock,
  fatal).

Findings are deduped by kind+message. The first fatal/error occurrence captures
a screenshot + `finding.json` + any crash markers into `findings/<n>/` and emits
a `finding` event to the action log. Severity ranks: `warning` < `error` <
`fatal`.

## Observability hooks (app-side)

Added under a small, always-safe surface:

- **Ring buffer** (`src-tauri/crates/telemetry/src/logging/mod.rs`): `VecDeque<RingLogEntry>`
  (`{seq, ts_ms, line}`), cap 500. `get_recent_logs_since(after_seq)` returns
  `{entries, latest_seq, dropped}` where `dropped` is true when the ring rotated
  past the checkpoint.
- **Rust panic hook** (`src-tauri/crates/telemetry/src/logging/panic_hook.rs`): writes a
  synchronous crash-marker JSON `crash-<pid>-<millis>.json` into the log
  directory (message, location, thread, `Backtrace::force_capture()`) and logs
  `PANIC: ...` to the ring. The marker is the durable signal (the file log layer
  can drop tail lines on abort). A thread panic leaves the app alive but the
  marker is fatal regardless.
- **Frontend global error capture** (`src/utils/globalErrorCapture.ts`):
  `window` `error` / `unhandledrejection` -> `console.error` with the
  `[unhandled-error]` / `[unhandled-rejection]` prefixes (the oracle contract),
  rides the console bridge into the ring, rate-limited.
- **Self-test command** (`src-tauri/src/commands/debug.rs`):
  `debug_trigger_panic(delay_ms)` panics on a spawned thread (app survives; hook
  fires). Inert unless `STALAGMIGHT_TEST_HOOKS=1` (the session sets it). This is
  the harness doctor primitive.

## Action-log JSONL contract (v1)

```jsonc
{"type":"session-start","v":1,"sessionId":"...","gitHead":"...","binaryPath":"...","display":":89","headed":false,"profileDir":"...","freshProfile":true}
{"type":"action","v":1,"seq":12,"ts":"...","tool":"click","mutating":true,"params":{...},"resolved":{"selector":"...","point":{"x":412,"y":333}},"outcome":{"ok":true,"durationMs":184},"result":{...},"artifacts":{"screenshot":"screenshots/0012-shot.png"},"post":{"view":"single","dialog":null,"activeTool":"select","projectName":"...","projectOpen":true}}
{"type":"event","v":1,"seq":40,"ts":"...","event":"finding|crash-detected|scenario-log","detail":{...}}
{"type":"session-end","v":1,"ts":"...","reason":"app_stop","actions":57}
```

`resolved` carries the concrete selector/coords the replay generator needs;
`eval`/`ipc`/`store_action` embed their full script/command/args. Generators
tolerate a truncated final line.

`mutating` says whether the call changed application state, decided by the tool
that made it - repro reports use it to pick the steps to reproduce, so a tool
declares it once at its definition instead of every consumer keeping a name
list. It stays `v:1` because it is optional: sessions recorded before the field
existed simply lack it, and `generate-repro-report.mjs` falls back to a frozen
legacy name list for those. A few calls are deliberately non-obvious -
`map_view` is mutating only in its setting form, `os_grant` is recorded as
mutating because every later `os_*` step depends on that consent, and
`os_screenshot` is observation despite the `os_` prefix.

## Replay + report

- **Replay**: `node scripts/generate-replay.mjs <sessionDir> [--out f] [--name n]
  [--assert-final]`. Interaction tools become real WDIO code using `resolved`,
  including `seed` (replayed as `loadProject`/`createProject`), `driver_script`
  (re-run with the same helpers), and `run_scenario` (re-run with the recorded
  seed/iterations). Map clicks re-project their recorded longitude/latitude,
  semantic waiters stay semantic, and test IDs are CSS-escaped. Observation
  tools become numbered comments; inter-action gaps > 250ms become bounded
  pauses; `--assert-final` turns the last
  `store_state` into a hard expect (skipped if the recorded value was
  truncated). No in-repo runner today (#549 retired the WDIO one; the #466
  replay-runner phase replaces it). Curated replays live under
  `e2e/agent/replays/`.
- **Report**: `node scripts/generate-repro-report.mjs <sessionDir>` ->
  `report.md` (steps, environment, oracle findings, evidence paths), with
  `$HOME`/session paths scrubbed for public issues.

## Artifacts layout

```
.agent-artifacts/sessions/<yyyymmdd-hhmmss-rand>/
  actions.jsonl        # the action log (JSONL contract above)
  screenshots/         # <seq>-<name>.png
  state/               # state snapshots
  findings/<n>/        # finding.json + screenshot.png + crash markers
  traces/              # fuzz traces, if a run recorded one
  pids                 # recorded PIDs (for scoped orphan reaping)
  tauri-driver.log     # continuous driver stderr/stdout
  replay.spec.ts       # if a replay was generated here
  report.md            # if a report was generated here
```

Gitignored; the 20 newest sessions are kept.

## Troubleshooting

- `app_start` fails `UND_ERR_INVALID_ARG` -> an error from webdriverio's undici
  HTTP layer. Check `HTTP_PROXY` / `NO_PROXY` in the environment first; on
  Node >= 26 the cause is the dispatcher pin (see "Runtime: the repo-pinned
  Node" above) - confirm `undici-dispatcher-pin.ts` is still imported ahead of
  `webdriverio`.
- `window.__E2E_TEST__ not present` -> the binary was built without `VITE_E2E=1`.
  `build: "auto"` should have caught that before attach (see the build stamp
  below); if you land here anyway, rerun `app_start` with `build: "force"`.
- The window never appears on screen -> that is the default: sessions run on a
  throwaway Xvfb even when a desktop display exists (GDK is pinned to X11 there,
  since a Wayland-preferring GTK would otherwise render to the real compositor).
  Pass `headed: true` to watch the app live on the real display.
- Orphan `tauri-driver` / `WebKitWebDriver` after an unclean server kill -> the
  next `app_start` scoped-reaps PIDs recorded in each session's `pids` file
  (verified by `/proc/<pid>/comm`); it never broadly `pkill`s.
- Second instance -> set `STALAGMIGHT_AGENT_BASE_PORT` to move the driver/native
  ports (default 4544 / 5544, probed +1 up to +10).
