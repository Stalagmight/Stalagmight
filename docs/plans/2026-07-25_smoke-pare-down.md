# E2E Smoke: where the time goes, and how to measure it

**Goal:** get the `E2E Smoke` CI job reliably under **7 minutes**.

**The finding this rests on: per-test setup (`beforeEach`) is ~60% of smoke test
time, not the assertions.** So the first move is to make setup cheaper (#295),
which costs no coverage - not to delete tests. Measured across five runs, two
very different trees, and both a buggy and a fixed reporter: 64-67% locally,
**60% on the hosted runner**.

---

## 1. Where the time goes

### The job

| Component | Measured |
|---|---|
| Whole `smoke` job | 5:59 - 7:20 |
| Setup (checkout -> pnpm install -> Playwright cache/deps) | 0:33 - 0:45, already at the floor |
| The Playwright step | ~90% of the job |

Every saved minute has to come out of test execution; there is nothing left in
job setup.

### ⚠ Effective parallelism is only ~1.9x - convert in seconds, not percentages

On a real run: **753.6s of test time in 393.9s of wall clock.** CI sets
`workers: '100%'` and the hosted runner is 2-core, so it is already saturated -
there is no free win from raising the worker count. The consequence that matters:

> **A second cut from aggregate test time removes only ~0.5s of job time.**

Every figure in the per-file table below is denominated in *test-time* seconds,
so read each one through that divisor: `gas-planning`'s 150.3s of hook time is
worth ~79s of job time, not 150s.

⚠ **Do not apply the divisor twice.** A *percentage* cut is not halved - the
test-time and wall-clock bases already differ by exactly this factor, so it
cancels. A 25% cut in test time is a 25% cut in the Playwright step, which is a
**~23%** cut in the job once the ~40s of fixed setup is added back. Halving that
too gives ~12%, which understates the payoff by 2x. (An earlier revision of this
doc did exactly that.)

A dev machine with 4+ workers still overstates the CI win in absolute seconds, so
re-derive on CI before believing a local improvement.

### Per file (hosted runner, 83 `@smoke` tests, 753.6s test time, 453.5s hook)

Percentages are rounded per row, so the column sums to 101.

| file | tests | % of test time | hook % |
|---|---:|---:|---:|
| gas-planning | 21 | **33%** | 61% |
| dive-plan | 7 | **13%** | 48% |
| map-tools | 3 | 5% | 25% |
| import-placement-flow | 3 | 4% | 59% |
| zoom-tool | 4 | 4% | 82% |
| undo-redo-dropdown | 4 | 4% | 85% |
| section-shot-editing | 3 | 3% | 91% |
| display-toggles | 2 | 3% | 67% |
| toolbar-tool-groups | 3 | 3% | 43% |
| dmp-import | 3 | 3% | 48% |
| auto-save-status | 2 | 3% | 78% |
| window-close-protection | 2 | 3% | 34% |
| the remaining 15 files | 26 | 20% | 6-100% |

Worst setup-to-value ratios, i.e. the files that are almost entirely boot cost:
`panel-reveal` 100%, `syntactic-highlighting` 99%, `corruption-recovery` 96%,
`section-shot-editing` 91%, `merge-closure-dialogs` 87%.

But **`gas-planning` is the real prize on absolute cost**: 150.3s of the 453.5s
total hook time, and 33% of all test time, in one file.

⚠ **A low hook % is not automatically a cheap file.** The reporter classifies by
Playwright step category, so setup done *inside the test body* is billed as body
time. `custom-renderer-health.spec.ts` reads as 6% hook, but it calls the same
`setupMockedPage` / `createProject` / `seedProjectWithSections` helpers as
everyone else - it just calls them in the test rather than in a `beforeEach`. It
is not a cheaper pattern to copy; moving a `beforeEach` into the test body
relabels cost without removing a millisecond of it. Judge a file by its total
seconds, and use hook % only to locate *where* the cost is charged.

---

## 2. The tooling

| File | What it is |
|---|---|
| `scripts/pw-timing-reporter.mjs` | Custom Playwright reporter. Uses `onStepEnd` to capture `category: 'hook'` durations, so per-test cost splits into hook vs body. Writes `PW_TIMING_OUTPUT` (default `playwright-report/timing.json`). Collapses retries into one row, charging all attempts. |
| `scripts/pw-timing-summary.mjs` | Renders that into a ranked markdown table (by file and by test). Accepts either the reporter's output (has hooks) or a built-in json report (reports hooks as `n/a`). Appends to `$GITHUB_STEP_SUMMARY` when set. |
| `scripts/pw-timing-{reporter,summary}.test.mjs` | Fixture tests, in the repo's pure-Node `*.test.mjs` style. `pnpm test:pw-timing` (renamed from `lint:pw-timing:test` in #576 - it is the reporter's self-test, not a lint or a time budget), run in the smoke job before the browser install. Each pins a bug that produced a confident WRONG number rather than an error. |
| `.github/workflows/e2e-smoke.yml` | Runs the reporter, prints the ranked table on the run summary page, uploads `timing.json` as the `smoke-timing` artifact (7-day retention). Originally marked temporary; kept permanently since #576 (E5-D3), because the smoke set's time budget is managed with it. |

### Reading a CI run's timing

```
gh run download <run-id> --repo Stalagmight/Stalagmight -n smoke-timing -D /tmp/t
node scripts/pw-timing-summary.mjs /tmp/t/timing.json --top 40
```

The table also renders directly on the run's summary page, including on green
runs - which is the point, since the `github` reporter prints per-test lines only
for failures and flakes.

### Reproducing locally

```
PW_RETRIES=0 PW_WORKERS=4 \
PW_TIMING_OUTPUT=<abs-path>/timing.json \
pnpm exec playwright test --grep @smoke \
  --reporter=line,./scripts/pw-timing-reporter.mjs

node scripts/pw-timing-summary.mjs <abs-path>/timing.json --top 25
```

⚠ **Always pass a second, human-readable reporter (`line`).** With only the
timing reporter there is no pass/fail output, and a fully-red run still produces
a plausible-looking table. Check the pass count before trusting any number. (The
summary now flags a non-green run explicitly, but the habit is cheaper than the
mistake: a stale `node_modules` once made every test time out in `beforeEach`,
and the resulting table read "hook = 98% of test time" - garbage that
superficially *confirmed* the thesis.)

---

## 3. Playwright traps this tooling had to solve

Keep these in mind before changing the reporter - each one produced a confident
wrong number rather than an error, which is the dangerous failure mode for a tool
whose output decides what gets deleted.

1. **The built-in `json` reporter does not emit `steps` at all.** So it cannot
   separate hook time from body time; a first attempt using it silently printed
   `hook: 0ms / 0%`, which reads as "setup is free" - the exact opposite of the
   truth, and would have pushed the work straight into deleting tests. The
   summary reports `n/a`, never `0%`, when the source cannot supply hook data.
   **Do not "simplify" the custom reporter away.**
2. **`Worker Cleanup` is a hook step that is not this test's setup.** Playwright
   emits it as a *sibling* of `Before Hooks`/`After Hooks` and charges it to
   whichever test happened to run last on that worker; `testInfo.duration`
   excludes it. Counting it as setup inflated the last test of every worker and
   every test adjacent to a failure - exactly the population a cost investigation
   looks at - and pushed rows past 100% hook, which is arithmetically impossible
   for a subset of test time. It is now tracked in its own `worker` field and
   excluded from hook totals. An unrecognized future hook title goes to an
   `unknown` bucket with a warning rather than silently defaulting into setup.
3. **A timed-out test bills the full 30s timeout**, so it sorts to the top of a
   cost ranking indistinguishable from expensive-but-real work. The summary flags
   any non-green run and marks each non-passing row.

---

## 4. Reliability context

- Measured 2026-07-26 over the preceding 60 days: **5 failures in 282 completed
  runs (1.8%)**, 2 of them on `master`. The suite is not flaky in aggregate, and
  that is the premise the whole pare-down rests on - a suite that fails often is
  a reliability problem, not a cost problem.
- The failures were 2026-06-18 (x3, the Windows/vitest-gate cluster), 2026-07-03
  (master, #250), and 2026-07-12 (`fix/audit-data-safety`).
- **The 2026-07-12 red was `dive-plan` catching a real regression** - 6 tests
  failing together, the "new gate reads state the mock never sets" class (#277).
  `dive-plan` is the best-evidenced catch in the suite: **de-flake it, do not
  delete it.** Its `enterPlan` helper (`dive-plan.spec.ts:60`) carries a
  sub-default 3000ms `dive-plan-bar` visibility timeout - the first thing to
  widen if it flakes.
- To re-derive. Two traps: the workflow sets `cancel-in-progress`, so cancelled
  runs must be excluded, and `--limit` is a run count rather than a date filter -
  set it well above the number of runs in the window or it silently truncates
  (200 undercounted this query by a third).
  ```
  gh run list --workflow "E2E Smoke" --limit 400 --json conclusion,createdAt \
    --jq '[.[] | select(.createdAt > "2026-05-27")
          | select(.conclusion == "success" or .conclusion == "failure")]
          | group_by(.conclusion) | map({(.[0].conclusion): length}) | add'
  ```

---

## 5. The plan

### Phase A - setup cost (attacks the ~60%, zero coverage loss)

- **A1. Done** - the instrumentation above. Keep it in CI until the pare-down
  lands; it is how Phase A gets measured. Remove it after.
- **A2. Cut the per-test re-seed - the single biggest lever.**
  Most smoke files re-seed a full mocked project in `beforeEach` per test:
  `page.goto('/')` boots the whole SPA, a project is opened through the real
  New-Project UI flow, then MapLibre initializes - all before any assertion runs.
  Several files pay it across many nested describes to run 2-4 smoke tests
  (`settings-dialog` and `display-toggles` have 9 `beforeEach` each,
  `undo-redo-dropdown` 7).
  ⚠ **Confirm each target actually runs that three-stage boot before counting it
  in.** Not every hook second is this seed: `scroll-containment.spec.ts` only
  installs mocks and calls `goto('/')` - no project, no MapLibre - so a cheaper
  shared seed does nothing for it. Group the files by the setup path they really
  execute and size the win from that subset, rather than applying a flat 50% to
  the whole 453.5s. Tracked as **#295**, which also records why the 30s
  global test timeout exists *because* setup is this heavy.
  A 50% cut in hook time is ~30% of test time, so ~30% off the Playwright step:
  a ~7:15 job becomes ~5:15. That clears the target on its own, before any test
  is dropped.
  ⚠ **Two constraints on any shared-fixture refactor:**
  `pnpm lint:e2e-parallel` gates `fullyParallel` safety and runs in the smoke
  job, so a worker-scoped fixture must keep it green. And per-test isolation is
  currently load-bearing: `e2e-playwright/mocks/tauri-ipc.ts` mutates
  `currentProject`'s arrays **in place**, so tests sharing a context will leak
  state into each other in a way this suite has never had to survive. Prove
  isolation rather than assuming it.
- **A3. Remove the remaining `waitForTimeout` sleeps in smoke files.** The
  `networkidle` half is already swept (#294/#300 - `grep -c networkidle
  e2e-playwright/` is 0). What remains: `undo-redo-dropdown` (400ms x2),
  `map-tools` (400ms), `back-button-navigation` (500ms), `import-placement-flow`
  (300ms x2), `settings-dialog` (x3), `zoom-tool` (a shared helper),
  `toolbar-tool-groups` (25ms), plus `helpers/app.ts` (100ms in the shared
  `placeAllSections` poll loop, hit by every import-flow test). Line numbers move;
  re-grep rather than trusting a list. **Done in #576 (E14):** the positive
  waits became condition waits, the deliberate negative-space settles carry a
  `// settle: <reason>` comment, `placeAllSections` (no callers since March) was
  deleted, and `pnpm lint:e2e-no-sleep` now fails on an unannotated sleep.

**Re-measure on CI after Phase A before cutting anything** - download the
`smoke-timing` artifact and diff the hook total. Remember the ~1.9x parallelism
divisor when converting an absolute test-time saving into a job-time saving.

⚠ **Take several samples on each side, not one.** Run-to-run spread on the
hosted runner is wide enough to swallow the effect being measured - the observed
job range is 5:59-7:20 - so a single before/after pair can show a 30% improvement
that is not there, or hide one that is. Getting this wrong decides whether Phase
B deletes real coverage for nothing. Re-run the smoke job a few times on both the
base and the change (`gh run rerun <id>`) and compare the medians.

### Phase B - test count, only as deep as the shortfall requires

Parked until Phase A is done and re-measured. If it is still needed:

- **`gas-planning.spec.ts` 21 -> ~6** is the one large cut: a third of all test
  time for a single feature. Keep: overlay renders, preset apply, cylinder edit,
  route pick, read-only guard, one readout. ⚠ ~61% of its cost is hook, so **A2
  shrinks this prize** - sequence A2 first and re-derive. **Done in #576 (E5),
  from 27 (it had grown) to exactly those six**, paying for one canary per
  uncovered critical feature (project delete, GPX import, the JSON/TML/PNG/
  SVG-PDF exports, detail panel, move, SpeleoDB managed file): 93 -> 81 `@smoke`
  tests, gas-planning 29% -> 7% of the set. The feature list and its gate live
  in docs/TESTING.md ("Critical features"); re-measure the job on CI.
- **Body-dominated files are the genuine Phase B candidates**, since A2 will not
  shrink them: `map-tools` (25% hook), `window-close-protection` (34%),
  `toolbar-tool-groups` (43%), `dmp-import` (48%).
- **Keep regardless:** `dive-plan` (proven catch), `project-crud` (critical
  path), `custom-renderer-health` (sole canary for the custom WebGL renderer,
  which has no fallback, and already cheap), and the data-safety cluster
  `error-handling` / `auto-save-status` / `window-close-protection` (guards the
  #92/#53 data-loss class). Note `auto-save-status` is 78% hook - **fix its
  setup, do not cut it.**
- **Do not bother cutting** the sub-1% files: the bottom third of the list carries
  real coverage for almost no time.
- Remove the **tag**, not the test - anything dropped from `@smoke` must still run
  in the nightly full suite.

### Rejected (with reasons, so they are not re-proposed)

- **Sharding smoke across runners.** Halves wall clock but **raises billed
  minutes** (duplicate ~40s setup + a second runner). Wrong lever when the cost
  being optimized is per-minute.
- **More Playwright browser caching.** Already cache-hit; worth ~10s.
- **Raising the worker count.** The runner is 2-core and `workers: '100%'`
  already saturates it.
- **Chasing the p90.** Wall-clock p90 is 23.8 min and max 115.7 min, but the
  timing API shows those same runs at 7.3-8.5 min of real execution - that tail is
  **runner queue time**, and no amount of test deletion touches it.

---

## 6. Gotchas

- **`@smoke` is a plain `--grep` on test titles**, not a describe-level tag. An
  unmatched test does not run, so it does not pay its own `beforeEach`. Since the
  dominant cost here *is* per-test `beforeEach`, **the test is the unit of cost,
  and removing a tag really does remove that test's setup.** Only file-level
  `beforeAll` and worker fixtures are shared, and those survive as long as one
  test in the file still matches - so a file drops to its floor, not to zero,
  until the last tag goes.
- Baseline: **83 `@smoke` tests across 27 files**. Count `test(` lines, not raw
  `@smoke` matches - `custom-renderer-health.spec.ts` also mentions the tag in a
  comment, so a bare `grep -c` overcounts.
- **`pnpm <script> -- <flags>` forwards `--` literally**, and Playwright reads it
  as a positional test filter ("No tests found"). Use `pnpm exec playwright test
  <flags>`. Keep the workflow's `--grep @smoke` in sync with `test:ui:smoke`.
- `pnpm lint:e2e-parallel` gates `fullyParallel` safety and runs in the smoke job.
- Docs changes do **not** trigger smoke/check (`docs/**` is in `paths-ignore`),
  but `.github/**` deliberately does - so a workflow edit gets a CI run for free.

## 7. Related issues

#23 (smoke/nightly split) · #294 (networkidle sweep, closed by #300) ·
#295 (app-boot `beforeEach` cost - **the Phase A2 ticket**) ·
#287 (zoom-tool ctrl-drag drift, suspected real bug)
