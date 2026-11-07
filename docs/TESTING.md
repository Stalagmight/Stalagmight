# Testing Guide

How the test tiers relate, which one a given test belongs in, and the contract a
UI component has to satisfy to be testable at all.

**Looking for a command or a pattern?** See
[Testing Recipes](testing-recipes.md) - exact commands per tier, per-framework
patterns, mock/fixture conventions, and the known traps.

A feature is not complete until it has coverage at the right tier. Picking the
wrong tier is the most common source of rework here, so start with the tier
model below.

## The tiers

| Tier | Framework | Environment | Covers | Speed |
|------|-----------|-------------|--------|-------|
| Unit (logic) | Vitest | `node` | Stores, utils, selectors (`*.test.ts`) | Milliseconds |
| Component | Vitest + Testing Library | `jsdom` | Render a React component, assert handlers/wiring (`*.test.tsx`) | Milliseconds |
| Playwright E2E | Playwright | Chromium, **mocked IPC** | UI flows across views (`e2e-playwright/`) | Seconds per test; full suite ~15 min |
| Backend | Rust `#[test]` | native | Rust logic, and the **real** file I/O / persistence paths (`src-tauri/src/` + `src-tauri/crates/*/src/`) | Fast |
| Agent harness | MCP + webdriverio + tauri-driver | **real Tauri app** | Journeys (release golden flows), stress scenarios, fuzz tiers, LLM-agent exploration, bug repro (`e2e/agent/`, on-demand) - see [Agent Harness](agent-harness.md) | Slow |

### The axis that actually matters: mocked IPC vs. real backend

Every *frontend* tier - Vitest unit, Vitest component, and all of Playwright -
mocks the Tauri backend. Nothing they assert proves a file was written, a
project was reopened, or a parser accepted a byte.

The real backend is covered by the **Rust tests**, which run in CI against real
`TempDir` files and need no display and no Tauri build. The command-layer tests
in `src-tauri/src/commands/command_tests.rs` are where the save/import
persistence pipeline is pinned.

One thing drives the real webview *and* the real backend together: the agent
harness (webdriverio underneath, see [agent-harness.md](agent-harness.md)). It
runs in no automated merge gate - journeys run per release candidate
([docs/uat](uat/ledger.md)) and on demand. Do not rely on it to catch anything
a PR gate should.

**Consequence:** if your change can fail on disk, in a parser, or across an app
restart, a Playwright test cannot cover it. Write a Rust test.

### Choosing a tier

| What you changed | Where the test goes |
|------------------|---------------------|
| Frontend logic (store, util, selector) | Vitest unit test (`*.test.ts`) |
| New UI component or dialog | Component test (`*.test.tsx`) for render + handler wiring, **plus** a Playwright spec for the flow that reaches it |
| A behavior that spans views, panels, or the map | Playwright spec |
| New Tauri IPC command | Rust unit tests for the logic, **plus** a mock handler in `e2e-playwright/mocks/tauri-ipc.ts` (a vitest tripwire fails until you add one - see recipes) |
| Backend logic (parser, model, solver) | Rust unit tests |
| Anything touching disk, persistence, or app restart | Rust command-layer test - no frontend tier can see it |
| Bug fix | Regression test at the **lowest** tier that can catch it. Prefer unit/component over E2E |

Component-local behavior ("clicking X calls Y", "dialog renders Z in state W")
belongs in a component test, not a Playwright spec: it runs in milliseconds
instead of seconds, and it fails with a stack trace instead of a screenshot.
Reach for Playwright when the assertion needs the real router, the real map, or
two panels talking to each other.

## Smoke vs. full (Playwright only)

The Playwright suite is split so timing-sensitive specs cannot block everyday
work:

| Tier | What | When it runs |
|------|------|--------------|
| **Smoke** | tests tagged `@smoke` (count them with `--grep @smoke`) | `e2e-smoke.yml`, on every PR/push its `paths-ignore` does not skip |
| **Full** | the entire Playwright suite | `nightly-e2e.yml`, 07:00 UTC daily, skipped when nothing merged in the last 2 days; upstream repo only (it needs the self-hosted runner - on a fork, run `pnpm test:ui` locally) |

- **Tag `@smoke` only if the test covers a critical path *and* is reliable.**
  Append the tag to the test title: `test('does X @smoke', ...)`. Selection is
  `playwright test --grep @smoke`; the convention here is to tag individual
  tests, not `describe` blocks, so the tagged set stays explicit. "Critical
  path" means the list below; a test outside it needs a reason to be tagged.
- A valuable but timing-sensitive test stays **untagged**. It still runs
  nightly.
- **Do not sleep.** A hard `page.waitForTimeout()` is the single biggest flake
  source. Use the shared waiters in `e2e-playwright/helpers/app.ts`, or
  web-first assertions / `expect.poll` / `toPass`. The one exception is a wait
  that proves something does NOT happen (a poll would pass on its first,
  pre-update read): mark it with a `// settle: <why>` comment directly above
  the call. `pnpm lint:e2e-no-sleep` (smoke job) fails on any other sleep:
  any `waitForTimeout` reference (a bound or destructured one too, or a key
  that spells it, `page['wait' + 'ForTimeout']`), a runner-side `setTimeout`
  (one outside `page.evaluate` and friends), a `timers/promises` import, or a
  wait whose failure is swallowed (`locator.waitFor({ timeout }).catch(() => {})`,
  or one awaited in a `try` whose `catch` does not always throw or assert - a
  catch that rethrows everything but the timeout swallows it too).
- Retries are configured in `playwright.config.ts` (2 in CI, 1 locally), so one
  blip does not abort a push. `PW_RETRIES=0` to fail fast while hunting a flake.

### Critical features (the smoke canary list)

The user paths a merge must never break, each with ONE named `@smoke` canary.
`pnpm lint:smoke-features` (smoke job and leak-scan) fails when a canary is no
longer an `@smoke` test - renamed, untagged, skipped or deleted - so re-point
the row in the same PR. A row passes when the spec has a non-skipped test whose
title contains `@smoke` and the canary fragment. The set is held to the smoke
job's time budget (under 7 minutes, `docs/plans/2026-07-25_smoke-pare-down.md`):
pay for a new canary by untagging a redundant test, never by deleting one - an
untagged test still runs nightly. The per-test timing report on every smoke run
(`pnpm test:pw-timing` pins its arithmetic) is how the budget is checked.

<!-- smoke-features:start -->
| Feature | Spec | Canary (title contains) |
|---|---|---|
| Create a project | `project-crud.spec.ts` | `should create project with valid name` |
| Save a project (Ctrl+S) | `project-crud.spec.ts` | `should respond to Ctrl+S` |
| Autosave status | `auto-save-status.spec.ts` | `an edit counts down, autosaves and returns to Saved` |
| A failed save is surfaced | `error-handling.spec.ts` | `should display error notification on save failure` |
| Delete a project | `globalview-delete-project.spec.ts` | `confirming deletes` |
| Import: Discovery DMP | `dmp-import.spec.ts` | `imports the selection through to the receipt` |
| Import: Ariane TML/TMLU | `tml-import.spec.ts` | `imports a TML from the unified picker` |
| Import: Walls WPJ/SRV | `walls-import-dialog.spec.ts` | `all surveys selected imports` |
| Import: GPX | `gpx-import-workflow.spec.ts` | `imports GPX from home screen` |
| Import: Compass MAK/DAT | `compass-import.spec.ts` | `imports a Compass MAK from the unified picker` |
| Export: CSV | `export-dialog.spec.ts` | `should execute export and show success` |
| Export: JSON | `export-dialog.spec.ts` | `sends export_json with a .json path` |
| Export: Ariane TML | `export-dialog.spec.ts` | `should continue TML export after unanchored warning confirmation` |
| Export: image (PNG) | `export-dialog.spec.ts` | `threshold slider updates the displayed value and is passed to export` |
| Export: image (SVG, PDF) | `export-dialog.spec.ts` | `exports SVG and PDF through export_image` |
| Detail panel | `detail-panel-links.spec.ts` | `clicking Start Station link changes selection to station` |
| Move a station | `move-carry-integrity.spec.ts` | `a normal drop still commits exactly one move` |
| Undo / redo | `undo-redo-dropdown.spec.ts` | `clicking item at index 2 executes undo_to` |
| Plan mode: gas overlay | `gas-planning.spec.ts` | `Plan mode shows the gas overlay` |
| Plan mode: saved dive plans | `dive-plan.spec.ts` | `Save as creates a plan and loads it` |
| SpeleoDB managed file | `speleodb-managed-file.spec.ts` | `remove link strips the file and opens it as a local project` |
| Map renders the survey (custom WebGL) | `custom-renderer-health.spec.ts` | `custom WebGL survey layer initializes and draws` |
<!-- smoke-features:end -->

Each canary asserts the feature's effect through the mock - the IPC call it
sends, with its arguments (`e2e-playwright/helpers/ipcRecorder.ts`), and the
state it leaves (a card gone, a receipt, a clean footer) - not merely that the
UI did not crash (#576 RV3-FE-7). This is enforced in review, not by
`lint:smoke-features`: the gate reads test titles only, so a weakened canary
still passes it (#576 RV4-FE-FEB-10).

The agent harness is **on-demand plus per-RC** and is in no gated tier above.
The retired WDIO suite's residual coverage lives in the journeys
(`pnpm agent:journeys`) and the `fractal-stress` scenario (#549).

## What CI gates

The authoritative list of commands to run before pushing is the **Pre-Push
Checklist in [`CLAUDE.md`](../CLAUDE.md)** - every gate the three workflows
below run is on it (`pnpm lint:prepush-checklist` in `leak-scan.yml` fails the
build when one is missing), and it is not repeated here.

Three workflows gate `master`:

- **`ci.yml` (`check`)** - the Rust and TypeScript gate: `cargo fmt --check`
  (root package + every workspace member), `cargo clippy --workspace` at deny
  level, the release-profile `cargo check --release` (the only compile with
  `debug_assertions` off), `tsc`, the test-ID contract, the agent-harness
  typecheck, the release-tooling suite (`scripts/test-release.sh`), the
  file-size / read-only-gating tripwires, the Vitest suite,
  `cargo test --workspace --features telemetry`, the ts-rs bindings drift check
  (`src-tauri/bindings/` must match what the tests just wrote) and the vendored
  `compass_data` tests. Each tripwire runs its own `:test` fixture first, so the
  rule itself is pinned. The order is lints first (fmt, clippy, the release
  check), then the pnpm gates, then the Rust tests; every step after clippy
  runs even after a lint failure (`if: !cancelled()`; the node steps also
  need a successful `pnpm install`), so one run reports the lints, tsc, the
  pnpm gates, Vitest and the Rust tests, and the job still fails.
- **`e2e-smoke.yml` (`smoke`)** - in order, all before the browser install so a
  cheap failure never pays for Chromium: the parallel-safety scanner
  (`pnpm lint:e2e-parallel`) and the no-unannotated-sleep gate
  (`pnpm lint:e2e-no-sleep`), each after its own `:test`;
  `pnpm typecheck:playwright`; the TS file-size ratchet (it measures
  `e2e-playwright/` too); `pnpm test:mock-contract`; the critical-feature
  canary check (`pnpm lint:smoke-features`, see "Critical features" above);
  `pnpm test:pw-timing` (the per-test timing reporter's self-test - the timing
  report is observability, not a time budget). Then the `@smoke` subset,
  Chromium-only against a built bundle served by `vite preview`. Every step can
  fail the job. The typecheck and mock-contract steps live here rather than in
  `check` because `check` path-ignores `e2e-playwright/**` - the very changes
  they guard.
- **`leak-scan.yml`** - `lint:private-leaks` + `lint:no-em-dash` over the whole
  tracked tree, `lint:third-party` (THIRD-PARTY-LICENSES.md against the
  hashes of the two lockfiles, the generator and its cargo-about config), and
  `lint:prepush-checklist` (the CLAUDE.md checklist lists every CI gate), and
  `lint:smoke-features` again (the canary list lives in `docs/`). The
  two whole-tree guards share one file reader (`scripts/lib/tracked-files.mjs`):
  a tracked symlink is scanned as its link text, the content git publishes for
  it, and is never followed, and every tracked file PATH is scanned as well as
  its content (an image named after a person is published too). It is
  the one workflow with **no** `paths-ignore`, because `check` and `smoke` both
  skip `**/*.md` and `docs/**` - exactly where a leak, a Unicode dash or a stale
  attribution bundle lands.

So a change that touches only markdown or `docs/` is gated by `leak-scan.yml`
alone, by design (#576 E18(i)): nothing `check` or `smoke` builds or tests reads
those files - the one markdown file a build bundles, the third-party attribution
bundle, is gated right here - and a malformed `CHANGELOG.md` is caught when a
release is tagged (`scripts/release-tag.sh` runs `scripts/release-notes.sh` on
it, see [releasing.md](releasing.md)).

One more, narrow, workflow: **`os-helper.yml`** runs `cargo fmt --check`, clippy
(`-D warnings`), the tests and a release build of the agent harness's standalone,
macOS-only OS helper crate (`e2e/agent/os/helper`) on a macOS runner, and only when
that crate or the workflow changes. **`telemetry-worker.yml`** is the same shape for
the standalone Cloudflare telemetry worker: it installs `telemetry-worker/` from its
own lockfile and type-checks it (`pnpm --ignore-workspace run typecheck`), only when
the worker or the workflow changes.

`check` and `smoke` use blocklist-style `paths-ignore`, so a new or unlisted path
defaults to *running*. Two edges are load-bearing and must not be widened:

- `check` must **not** ignore all of `e2e/**`. Rust tests read
  `e2e/fixtures/*.dmp`, so a fixture edit has to run `check`.
- `check` must **not** ignore `e2e/agent/**`. `pnpm typecheck:agent` is the
  harness's only automated gate.

The full Playwright suite runs nightly, not on every change.

## Test conventions

- **Name the behavior, not the number.** `should display error when project name
  is empty`, not `test1`.
- **Each test sets up its own state** and depends on no other test. Playwright
  runs `fullyParallel` at multiple workers; a spec that leaks shared state (its
  own server, a fixed-path disk write, forced serial mode) fails
  `pnpm lint:e2e-parallel`. See the recipes for the full rule set.
- **Assert something specific.** `await expect(page.locator('h1')).toHaveText('Stalagmight')`,
  not `expect(true).toBeTruthy()`.
- **Wait for a condition, never for a duration.**
- **Put context in the assertion** so a failure message is readable on its own.

## UI component testability requirements

Tests locate elements through `data-testid` and ARIA. A component that ships
without them is untestable at the E2E tier, so these are requirements, not
suggestions.

### Checklist for a new component

- [ ] Dialogs have `role="dialog"` and `aria-modal="true"`
- [ ] Dialog titles have an `id` referenced by the dialog's `aria-labelledby`
- [ ] Every interactive control has a stable, unique `data-testid`
- [ ] Controls rendered in a loop include a stable item key in the test ID
- [ ] Icon-only buttons have `aria-label`

`pnpm lint:test-ids:test && pnpm lint:test-ids` gates the statically provable
parts of these requirements. The scanner rejects interactive JSX without a test
ID, duplicate literal sibling IDs, and literal IDs inside `map`, `flatMap`, and
`Array.from` render callbacks. Dynamic expressions and helper-component reuse
still require review because static analysis cannot prove their runtime identity.
In particular, a custom component's call site (`<SectionRow onClick={...}>`, a
capitalized tag not in the scanner's `INTERACTIVE_TAGS`) is deliberately not
checked: the component's own definition is, so it must tag the control it
renders. A wrapper that is itself a plain control belongs in `INTERACTIVE_TAGS`.

### Dialogs (mandatory shape)

```tsx
<div
  role="dialog"
  aria-modal="true"
  aria-labelledby="dialog-title-id"
  data-testid="example-dialog"
>
  <h2 id="dialog-title-id">Dialog Title</h2>
</div>
```

`data-testid` is what E2E targets; the ARIA attributes are what screen readers
and the focus trap need. Both are required.

### `data-testid` naming

| Element type | Convention | Example |
|--------------|------------|---------|
| Main views | `<view>-view` | `global-view` |
| Buttons | `<action>-button` | `new-project-button` |
| Dialogs | `<name>-dialog` | `new-project-dialog` |
| Form inputs | `<field>-input` | `project-name-input` |
| Statistics | `stats-<metric>` | `stats-total-length` |
| Detail panel links | `<type>-<sectionId>-<position>-link` | `station-2-5-link`, `shot-2-3-link` |
| Section links | `section-<sectionId>-link` | `section-2-link` |
| Center-map buttons | `center-<type>-button` | `center-station-button` |
| Detail views | `<type>-detail` | `station-detail`, `section-detail` |

Keep an id stable once a test uses it. Renaming one is a breaking change to the
suite.

### ARIA

| Attribute | Use for |
|-----------|---------|
| `role="dialog"` | Modal dialogs (required) |
| `aria-modal="true"` | Dialogs that block interaction |
| `aria-labelledby` | Connecting a dialog to its title |
| `aria-label` | Icon-only buttons, and any container whose meaning is visual (stats, links, center buttons) |
| `aria-expanded` | Collapsible panels |
| `aria-disabled="true"` | A control that renders but cannot be activated (e.g. a link to a missing target) |

Containers whose meaning is carried by layout - stats blocks, detail-panel
links, center buttons - carry a descriptive `aria-label` spelling out the value
("Center map on station 2:5"). Tests read those labels, so they double as a
stable assertion surface.

### Selector priority

1. `data-testid` - most reliable
2. `role` + `aria-*`
3. Semantic elements (`button:has-text("New Project")`)
4. CSS classes - least reliable, avoid

## Coverage

Every tier can be instrumented, and the raw output merges into one report under
`docs/coverage/` (gitignored). The commands are in the
[recipes](testing-recipes.md#coverage).
