# E2E Smoke pare-down - handoff

## START HERE (handoff, 2026-07-26)

**Status: instrumentation is MERGED and live in CI** (PR #312, master
`93badc8e`). Measurement phase is done. **Next action is Phase A2 (issue #295):
cut per-test app-boot cost.** Nothing is in flight; the working tree is clean and
there is no open branch for this work.

**What you can now do that the previous instance could not:** every `E2E Smoke`
run prints a ranked hook-vs-body cost table on its run summary page, and uploads
`timing.json` as the `smoke-timing` artifact (7-day retention). So you can
measure a change on the real runner instead of extrapolating from an M1:

```
gh run download <run-id> --repo Stalagmight/Stalagmight -n smoke-timing -D /tmp/t
node scripts/pw-timing-summary.mjs /tmp/t/timing.json --top 40
```

**The four numbers that matter** (measured on the hosted runner, run
30205889359, 83 tests):

| | |
|---|---|
| Job wall clock | 7:20 (target: under 7:00; master runs 5:59-7:20) |
| Test step | 396s of that; all setup is 33s and already at the floor |
| Hook (setup) share | **60%** of test time - the whole thesis |
| Effective parallelism | **~1.9x** (753.6s test time in 393.9s wall) |

**Read the parallelism number before you plan anything.** The hosted runner is
2-core and `workers: '100%'` already saturates it, so **a second saved in test
time buys only ~half a second of job time.** A 25% test-time cut is a ~12% job
cut. Local M1 wall-clock deltas (4 workers) overstate the CI win by ~2x. Size
the work against wall clock, not test time - this is the single easiest way to
over-promise on Phase A.

**Where Phase A2's prize is**, worst setup-ratio first, from the CI run:

| file | tests | hook | hook % |
|---|---:|---:|---:|
| panel-reveal | 1 | 7.8s | 100% |
| syntactic-highlighting | 1 | 8.2s | 99% |
| corruption-recovery | 2 | 8.0s | 96% |
| section-shot-editing | 3 | 23.9s | 91% |
| merge-closure-dialogs | 2 | 13.1s | 87% |
| undo-redo-dropdown | 4 | 23.1s | 85% |
| zoom-tool | 4 | 22.4s | 82% |
| settings-dialog | 3 | 14.8s | 82% |
| auto-save-status | 2 | 16.6s | 78% |
| **gas-planning** | **21** | **150.3s** | **61%** |

`gas-planning` is not the worst *ratio* but is by far the largest *absolute*
block (150s of the 453s total hook time, 33% of all test time). It is the single
biggest Phase A2 win and also the Phase B target - fix its setup first and
re-derive before deciding whether B is still needed.

`custom-renderer-health.spec.ts` (6% hook) is the counter-example to copy: it
drives the app directly instead of going through the shared per-test seed.

**Do not** start by deleting tests. Phase B is deliberately parked (section 6).

---

**Goal:** get the `E2E Smoke` CI job reliably under **7 minutes**.
**Status (2026-07-26):** rebased onto ToT `2ab85459`, all measurements re-derived
on the post-#297 tree. No test has been cut yet. **Phase B is parked** until
Phase A is done and re-measured.

**Landed:** PR #312, merged 2026-07-26 as master `93badc8e`. The branch
`chore/smoke-timing-instrumentation` is spent - do not resume it. Start Phase A
from a fresh branch off `origin/master`.

---

## 0. What the 2026-07-26 rebase changed

182 commits landed between the branch point (`099d86d1`) and ToT. Five of them
move this plan:

| Landed | Effect on this plan |
|---|---|
| **#300** (networkidle sweep) MERGED | `grep -c networkidle e2e-playwright/` is now **0**. Phase A3's networkidle half is **done**. It also cut the CI test step by ~60-90s on its own - see the new baseline below. |
| **#297** (custom WebGL renderer) MERGED | The "defer map trims until post-#297" hold is **lifted**. |
| **#292** (dive-plan flake) CLOSED `NOT_PLANNED` | Subsumed by #300 - the flake mechanism was the `networkidle` wait, now gone. **Phase A4 is a no-op**; the local fix branch was deleted unpushed. |
| **#293** em-dash sweep | New CI gate `pnpm lint:no-em-dash` on the leak-scan job. This branch passes it. |
| **#305** (renderer health canary) MERGED | Adds one `@smoke` test. Baseline is now **83 across 27 files**. It is a **keeper** - see section 3. |

Two handoff assumptions were **wrong** and are corrected here:

- "#297 only trivially touches `gas-planning` (+10/-3)." It was **+189/-73** -
  a substantial rewrite. The file still has **21** `@smoke` tests, so the Phase B
  target is unchanged, but every line number in the old plan is stale.
- "Unknown: which `test()` blocks keep their `@smoke` tag in the rewritten specs."
  Resolved: **none changed.** #297's spec rewrites preserved every tag - the count
  held at exactly 82 across 26 files, identical to the pre-#297 tree, and only
  #305 moved it (to 83/27).

---

## 1. Where the time actually goes (re-derived on ToT)

### CI baseline has MOVED - the gap is much smaller than the old plan assumed

Master runs since #297/#300 (jobs API, runs 30185182612 / 30149840595 /
30145913400 / 30139667882):

| Component | Old (pre-#300) | **ToT** |
|---|---|---|
| Whole `smoke` job | 7:00 - 8:15 | **5:59 - 7:07** |
| Setup (checkout -> pnpm install -> PW cache/deps) | 0:35 - 0:44 | 0:35 - 0:45 (unchanged) |
| `pnpm test:ui:smoke` step | 6:19 - 7:36 | **5:01 - 6:24 (301-384s)** |

**The job already hits the target on a good run and misses it on a bad one.** The
remaining ask is roughly a **20-25% cut off the test step** (to ~280s for margin),
not the 25-35% the original plan scoped. Reducing *variance* now matters as much
as reducing the mean.

Setup remains at the floor; every saved minute still has to come out of test
execution.

### First real-runner sample (PR #312, run 30205889359)

The instrumentation landed and produced this on the hosted runner - **83 tests,
test-time total 753.6s, hook 453.5s = 60%, wall clock 393.9s** (job 7:20, of
which the test step is 396s and all setup is 33s).

Three things this says that the M1 could not:

1. **The finding transfers.** Hook share is 60% on the real runner vs 64-67%
   locally. Lower, but the same order: setup still dominates, and Phase A is
   still the right first move.
2. **`gas-planning` is WORSE on CI than locally - 33% of test time** (vs 27-31%
   on the M1), 248s of the 754s, at 11.8s per test. It is the only file above
   13%. Phase B's target is confirmed, and if A under-delivers this is where to
   go.
3. **⚠ Effective parallelism is only ~1.9x** (753.6s of test time in 393.9s of
   wall clock), even though CI sets `workers: '100%'`. The hosted runner is
   2-core, so it is already saturated - there is no free win from raising the
   worker count, and, more importantly, **every second cut from test time buys
   only ~half a second of wall clock.** A 25% test-time cut is a ~12% job cut.
   Size Phase A's target against wall clock, not test time. (The M1 runs 4
   workers, so local wall-clock deltas overstate the CI win by ~2x.)

### Local instrumented runs (4 workers, retries off, M1)

| Metric | Pre-rebase | ToT run 1 | ToT run 2 (post-#305) | **run 3 (fixed reporter)** |
|---|---|---|---|---|
| Tests | 82 passed | 82 passed | 83 passed | **83 passed** |
| Test-time total | 432.1s | 351.4s | 339.8s | **590.5s** |
| Hook (setup) time | 277.4s = **64%** | 234.5s = **67%** | 223.2s = **66%** | **377.0s = 64%** |
| Wall clock | 115.1s | 93.2s | 90.1s | **154.6s** |

Run 3 is the authoritative one: it is the only sample taken with the corrected
hook attribution (see section 2). Its absolute seconds are inflated because the
machine was running other work concurrently - which is exactly why the *ratio*
is the number to carry, not the seconds.

The hook share is stable at **64-67% across four local samples**, two very
different trees, and both the buggy and fixed reporters - and **60% on the
hosted runner** (above). That is the one number this plan rests on, and it is
not noise.

⚠ Run-to-run variance is ~30% and local wall clock is ~4x faster than the hosted
runner. The *ranking* and the *hook share* transfer; the seconds do not.

### The finding that drives the plan SURVIVED the renderer swap

**Setup (beforeEach) is 66% of smoke test time.** Per file (ToT run 2):

| File | tests | % of test time | hook % | avg/test |
|---|---:|---:|---:|---:|
| gas-planning.spec.ts | 21 | **28%** | 71% | 4.5s |
| dive-plan.spec.ts | 7 | **10%** | 64% | 4.9s |
| import-placement-flow.spec.ts | 3 | 5% | 53% | 5.8s |
| back-button-navigation.spec.ts | 2 | 4% | **85%** | 6.8s |
| undo-redo-dropdown.spec.ts | 4 | 4% | **86%** | 3.4s |
| auto-save-status.spec.ts | 2 | 4% | **84%** | 6.6s |
| zoom-tool.spec.ts | 4 | 4% | **87%** | 3.3s |
| window-close-protection.spec.ts | 2 | 4% | 32% | 6.6s |
| toolbar-tool-groups.spec.ts | 3 | 4% | 38% | 4.2s |
| dmp-import.spec.ts | 3 | 4% | 39% | 4.0s |
| map-tools.spec.ts | 3 | 4% | 24% | 4.0s |
| section-shot-editing.spec.ts | 3 | 3% | **94%** | 4.0s |
| the remaining 15 files | 26 | 22% | 4-99% | - |

Movement vs the pre-rebase table: `back-button-navigation` and `auto-save-status`
climbed into the top group (both ~85% hook - the worst setup-to-value ratio in
the suite alongside `section-shot-editing` at 94%). `gas-planning` fell 36% ->
28% but is still by far the largest single block, at more than twice the next
file.

**#305's canary is cheap and well-built:** 4.3s, only 4% hook (it drives the app
directly rather than through the shared per-test seed). It is the model for what
Phase A should make the rest of the suite look like.

**Cutting tests attacks the 31%. Fixing setup attacks the 67%. Fix first.**

The small files remain not worth cutting: the bottom 12 files carry 21 tests for
15% of test time, and several are the data-safety guards.

---

## 2. What landed in #312 (now on master)

| File | What it is |
|---|---|
| `scripts/pw-timing-reporter.mjs` | Custom Playwright reporter. Uses `onStepEnd` to capture `category: 'hook'` durations, so per-test cost splits into hook vs body. Writes `PW_TIMING_OUTPUT` (default `playwright-report/timing.json`). Collapses retries into one row, charging all attempts. |
| `scripts/pw-timing-summary.mjs` | Renders that into a ranked markdown table (by file and by test). Accepts either the reporter's output (has hooks) or a built-in json report (reports hooks as `n/a`). Appends to `$GITHUB_STEP_SUMMARY` when set. |
| `.github/workflows/e2e-smoke.yml` | Runs `--reporter=github,./scripts/pw-timing-reporter.mjs`, prints the ranked table on the run summary page, uploads `timing.json` as the `smoke-timing` artifact (7-day retention). Marked **TEMPORARY**; remove when the pare-down lands. |

| `scripts/pw-timing-reporter.test.mjs`, `scripts/pw-timing-summary.test.mjs` | Fixture tests for the two above, in the repo's pure-Node `lint:*:test` style. Wired as `pnpm lint:pw-timing:test` and run in the smoke job before the browser install. Each pins a bug that produced a confident WRONG number rather than an error - see below. |

Gates at merge: `lint:e2e-parallel` clean (104 files), `lint:ts-file-size` clean
(354 files), `lint:private-leaks` clean, `lint:no-em-dash` clean,
`lint:pw-timing:test` 28 passed. CI on the merged PR: `check` 6m20s pass, `smoke`
7m20s pass (83 passed, 0 flaky), `leak-scan` pass.

### Two attribution bugs the pre-PR review caught (both now fixed + pinned)

Both produced a confident, plausible number rather than an error, which is the
dangerous failure mode for a tool whose output decides what gets deleted:

1. **`Worker Cleanup` was counted as setup time.** Playwright emits it as a
   *sibling* of `Before Hooks`/`After Hooks` and charges it to whichever test ran
   last on that worker; `testInfo.duration` excludes it. Counting it as `before`
   inflated the last test of every worker and every test adjacent to a failure -
   exactly the population a cost investigation looks at - and could push a row's
   hook share above 100%. That was the visible tell: the all-timed-out run
   described in section 2 reported a `hook %` of **101-112% on 24 of 26 files**
   (`gas-planning` 101%, `back-button-navigation` 102%, `theme` 104%,
   `scroll-containment` 112%) - arithmetically impossible for setup time that is
   a subset of test time, and a signal that was there to be read before the
   numbers were trusted. Now tracked in its own `worker` field and excluded from
   hook totals. An unrecognized future hook title goes to a separate `unknown`
   bucket with a warning rather than defaulting into setup.
2. **Test status never reached the output.** A timed-out test bills the full
   30s timeout, so it sorted to the top of the ranking looking like an expensive
   but legitimate test. The summary now flags any non-green run and marks each
   non-passing row.

Neither bug affects the headline finding: the 64-67% hook share was measured on
green runs, where `Worker Cleanup` is a small tail. But re-measure with the fixed
reporter before acting on any per-file number.

### Why a custom reporter (do not undo this)

**Playwright's built-in `json` reporter does not emit `steps` at all.** The first
attempt used it and silently printed `hook: 0ms / 0%` - which reads as "setup is
free," the exact opposite of the truth, and would have pushed the work straight
into deleting tests. The summary script now reports `n/a` rather than `0%` when
the source cannot supply hook data.

### Reproduce the measurement

```
PW_RETRIES=0 PW_WORKERS=4 \
PW_TIMING_OUTPUT=<abs-path>/timing.json \
pnpm --dir <repo> exec playwright test --grep @smoke \
  --reporter=line,./scripts/pw-timing-reporter.mjs

node scripts/pw-timing-summary.mjs <abs-path>/timing.json --top 25
```

⚠ Always pass a **second, human-readable reporter** (`line`). With only the
timing reporter there is no pass/fail output, and a fully-red run looks like a
valid measurement. That happened on the first ToT attempt: a missed
`pnpm install` after the 182-commit rebase left `@mapbox/tiny-sdf` unresolved,
every test timed out in `beforeEach`, and the table read "hook = 98% of test
time" - garbage that superficially confirmed the thesis. The summary now flags a
non-green run explicitly, but **check the pass count anyway.** (Also: `pnpm
install` run under the Bash sandbox silently no-ops here; run it with the sandbox
disabled.)

---

## 3. Inbound work

- **#305 MERGED** (`2ab85459`) - `e2e-playwright/custom-renderer-health.spec.ts`,
  the WebGL-health canary for the now-unfallback-able custom renderer. It closes
  the automated-coverage gap this plan flagged against #297, so it is a
  **keeper**: it must not be a pare-down target, and any shared-fixture refactor
  in Phase A has to accommodate it.
  ⚠ It is the first spec whose *prose* contains the string `@smoke` (line 44, a
  comment). A bare `grep -c @smoke` now overcounts by one - count `test(` lines,
  or trust the reporter.
- **#307 MERGED** (mock shot-id fidelity / section-id bound / rigid-group datum
  guard). It touched `e2e-playwright/mocks/tauri-ipc.ts`, so re-read the mock
  before relying on any older note about it.
- **PR #291 / `goblinland`** is unrelated (zero `e2e-playwright` files). There is
  a worktree at `.claude/worktrees/goblinland-291`; leave it alone.
- Nothing else open touches `e2e-playwright/**`, so Phase A starts on a clear
  field - but re-check `gh pr list` before branching, since the e2e mock and the
  specs are high-traffic files.

---

## 4. Reliability

- Over the 60-day window: **61 green, 1 red** in 62 smoke runs. Not flaky in
  aggregate. All 25 runs in the last 48h are green or cancelled-by-supersede.
- The one recurring flake, `dive-plan.spec.ts:220`, is **resolved by #300**
  (#292 closed as subsumed). Its `enterPlan` helper still carries a
  sub-default 3000ms `dive-plan-bar` visibility timeout
  (`dive-plan.spec.ts:60`) - harmless now that the 30s `networkidle` stall is
  gone, but it is the first thing to widen if that line flakes again.
- **The single red run was `dive-plan` catching a real regression** - 6 tests
  failing together, the #277 "new gate reads state the mock never sets" class.
  dive-plan is the best-evidenced catch in the suite. **De-flake it; do not
  delete it.**
- Tool: `python3 ~/tools/ci_flake_report.py --repo Stalagmight/Stalagmight
  --days 30 --workflows "E2E Smoke"`. (`~/tools/gh_actions_report.py` errors with
  `IncompleteRead` on this repo's run history.)

---

## 5. The plan

### Phase A - setup cost (attacks the 60%, zero coverage loss)

- **A1. DONE and MERGED** - instrumentation (#312). Leave it in CI until the
  pare-down lands; it is how Phase A gets measured. Remove it after.
- **A2. Cut the per-test re-seed. This is the single biggest lever, and the next
  thing to do.** Every smoke file re-seeds a full mocked project in `beforeEach`
  per test: `page.goto('/')` boots the whole SPA, then a project is opened
  through the real New-Project UI flow, then MapLibre initializes - before any
  assertion runs. Several files pay it across many nested describes to run 2-4
  smoke tests (`settings-dialog` and `display-toggles` have 9 `beforeEach` each,
  `undo-redo-dropdown` 7). Tracked as **#295**, which already has the measured
  case (~90% of a `dive-plan` test is shared setup) and the argument that the
  30s global timeout exists *because* setup is this heavy.
  Ranked targets are in the START HERE table above (use those CI numbers, not
  the local ones further down). `custom-renderer-health.spec.ts` (6% hook) is
  the shape to aim for: it drives the app directly rather than through the
  shared seed.
  ⚠ **Two constraints on any shared-fixture refactor:**
  `pnpm lint:e2e-parallel` gates `fullyParallel` safety and runs in the smoke
  job - a worker-scoped fixture must keep it green. And per-test isolation is
  load-bearing for the mock: `e2e-playwright/mocks/tauri-ipc.ts` mutates
  `currentProject`'s arrays **in place**, so tests that share a context will
  leak state into each other in a way the current suite has never had to
  survive. Budget for that, and prove isolation rather than assuming it.
- **A3. Remove the remaining `waitForTimeout` sleeps in smoke files.** The
  networkidle half is done (#300). What is left, re-derived on ToT:
  `undo-redo-dropdown:705,825` (400ms x2), `map-tools:449` (400ms),
  `back-button-navigation:227` (500ms), `import-placement-flow:156,288` (300ms x2),
  `settings-dialog:67,120,794`, `zoom-tool:62` (shared helper),
  `toolbar-tool-groups:51` (25ms), plus `helpers/app.ts:316` (100ms, in the
  shared `placeAllSections` poll loop - hit by every import-flow test).
  Follow-up issue: **#294**.
- **A4. Dropped** - #292 is closed as subsumed by #300. No work.

**Re-measure after Phase A before cutting anything.** Measure on CI, not the M1:
download the `smoke-timing` artifact from the PR's own smoke run and diff the
hook total against the run 30205889359 baseline in START HERE. Remember the
~1.9x parallelism divisor when converting a test-time saving into a job-time
saving.

### Phase B - test count, only as deep as the shortfall requires

- **`gas-planning.spec.ts` 21 -> ~6** remains the one large cut: 28% of test time
  for a single feature. Keep: overlay renders, preset apply, cylinder edit, route
  pick, read-only guard, one readout. ⚠ 71% of its cost is hook, so **A2 shrinks
  this prize** - sequence A2 first and re-derive rather than assuming ~65s.
- **Map/canvas trims are now unblocked** (#297 merged). But re-derive from the
  merged tree: the rewritten specs' `@smoke` sets are unchanged in count, and
  `map-tools` (24% hook) / `window-close-protection` (32%) /
  `toolbar-tool-groups` (38%) / `dmp-import` (39%) are body-dominated, so A2 will
  not shrink them - they are the genuine Phase B candidates.
- **Keep regardless:** `dive-plan` (proven catch), `project-crud` (critical path),
  `custom-renderer-health` (sole renderer canary, and already cheap), and the
  data-safety cluster `error-handling` / `auto-save-status` /
  `window-close-protection` (guards the #92/#53 data-loss class). Note
  `auto-save-status` is 84% hook - **fix its setup, do not cut it.**
- **Do not bother cutting** the sub-1% files.
- Remove the **tag**, not the test - everything dropped from `@smoke` must still
  run in the nightly full suite.

### Rejected (with reasons, so they don't get re-proposed)

- **Sharding smoke across runners.** Halves wall-clock but **raises billed
  minutes** (duplicate ~40s setup + a second runner). Wrong lever when the cost
  being optimized is per-minute.
- **More Playwright browser caching.** Already cache-hit; worth ~10s.
- **Chasing the p90.** Wall-clock p90 is 23.8 min and max 115.7 min, but the
  timing API shows those same runs at 7.3-8.5 min of real execution - that tail is
  **runner queue time**, and no amount of test deletion touches it.

---

## 6. Decisions taken (2026-07-26)

1. **Land the instrumentation.** Local M1 estimates cannot resolve a 5:59-7:07
   variance band; only repeated real-runner samples can. "We can't change what we
   don't measure." -> this branch goes to PR.
2. **Do Phase A. Park Phase B.** The goal stands even though ToT already
   straddles 7:00, because suite time does not stay down on its own - it only
   ratchets up. Phase A is zero-coverage-loss, so it is worth doing regardless of
   the current gap. Revisit B only after A is done and re-measured.
3. **#305 and #307 have both landed**, so a Phase A fixture refactor must
   accommodate the renderer canary from the start, and should re-read
   `tauri-ipc.ts` rather than trusting older notes about it.

---

## 7. Gotchas for whoever picks this up

- **After any rebase, `pnpm install` before measuring** - and check the pass count
  on the run. See the section-2 warning: a stale `node_modules` produces a
  plausible-looking, entirely wrong timing table.
- **`pnpm install` run under the Bash sandbox can silently no-op** - it printed a
  version banner and installed nothing, which is how the stale-`node_modules`
  wrong-table incident happened. Run it with the sandbox disabled and read the
  `+N -N` package line before believing it worked.
- **Do not pipe a Playwright run through `tail`/`head` and read the exit code** -
  the pipeline's status is the last command's, so a fully-red run reports
  `EXIT=0`. Redirect to a file and inspect it instead.
- **Do not edit files while a review subagent is reading the tree.** One aborted
  mid-review with "TREE MOVED" and had to be re-run against an explicit commit
  range, costing a full review cycle. Either finish the fixes first, or brief the
  reviewer to pin `git diff master...HEAD` at a stated SHA.
- **Run Playwright with the Bash sandbox DISABLED** - the dev server cannot bind
  :1420 otherwise (EPERM). Check the port is free first; a second clone's dev
  server on :1420 would silently serve the wrong code to the tests.
- **`git checkout` / `reset` touching `.claude/**` fails under the sandbox**
  ("Operation not permitted") and can leave the worktree half-updated with the
  ref behind. Re-run the git command with the sandbox disabled.
- **`pnpm <script> -- <flags>` forwards `--` literally**, and Playwright reads it
  as a positional test filter ("No tests found"). Use
  `pnpm exec playwright test <flags>`. Keep the workflow's `--grep @smoke` in sync
  with the `test:ui:smoke` script.
- **`@smoke` is a plain `--grep` on test titles**, not a describe-level tag. But
  Playwright still runs a whole file's fixtures if it has >=1 matching test.
- Baseline count: **83 `@smoke` tests across 27 files** on ToT. Count `test(`
  lines, not raw `@smoke` matches - `custom-renderer-health.spec.ts` mentions the
  tag in a comment too.
- `pnpm lint:e2e-parallel` gates `fullyParallel` safety and runs in this job - a
  shared-fixture refactor must keep it green.
- `pnpm lint:no-em-dash` is a leak-scan gate since #293 - ASCII hyphens only in
  new prose.
- Docs changes do **not** trigger smoke/check (`docs/**` is in `paths-ignore`), but
  `.github/**` deliberately does - so a workflow edit gets a CI run for free.
- Repo rules: the pre-PR review before any PR; do not self-merge.

## 8. Related issues

#23 (smoke/nightly split) · #294 (networkidle sweep, closed by #300) ·
#295 (app-boot beforeEach cost - **the Phase A2 ticket**) · #305 (renderer health
canary, merged) · #307 (mock fidelity, open) · #287 (zoom-tool ctrl-drag drift,
suspected real bug)
