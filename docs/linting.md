# Linting & Formatting Policy

This repo runs a deliberately strict Rust lint + format policy whose primary job
is to **catch AI-generated-code smells** before they land. Both gates run in the
CI `check` job and must be green to merge. The frontend's gates are at the
end: [Frontend: ESLint](#frontend-eslint) and the shared TypeScript compiler
options, [TypeScript: one shared base](#typescript-one-shared-base).

| Gate | Command | Config |
|------|---------|--------|
| Format | `cargo fmt --manifest-path src-tauri/Cargo.toml --check`, then `for m in src-tauri/crates/*/Cargo.toml; do cargo fmt --manifest-path "$m" --check; done` | `src-tauri/rustfmt.toml` (stable defaults, `edition` pinned; applies to every member) |
| Lint | `cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --features telemetry --all-targets` | `src-tauri/Cargo.toml` → `[workspace.lints.clippy]` / `[workspace.lints.rust]`, inherited by every member via `[lints] workspace = true` |

Run `cargo fmt` and `cargo clippy` locally before pushing (they're in the
[Pre-Push Checklist](../CLAUDE.md)).

## Formatting: rustfmt scope and adoption

`cargo fmt` is the formatter for the **`src-tauri` workspace** (the
`stalagmight` app package plus the eight member crates under
`src-tauri/crates/`), enforced in CI: the `check` job runs
`cargo fmt --manifest-path src-tauri/Cargo.toml --check` and then the same
check in a loop over `src-tauri/crates/*/Cargo.toml`, failing on any drift.
Run `cargo fmt` (root + the same loop, without `--check`) to fix formatting
before pushing; the `--check` forms in the Pre-Push Checklist then pass.
Config is `src-tauri/rustfmt.toml` - stable rustfmt defaults, only `edition`
pinned; do **not** add nightly-only keys (they'd diverge local vs CI).

**Why a loop and not `--all`:** `cargo fmt --all` formats every *path
dependency* as well as the workspace members, which would reformat the
deliberately-unformatted vendored `src-tauri/vendor/compass_data` crate. The
per-manifest loop touches exactly the workspace's own packages.

**Scope:** the gate formats the `stalagmight` package and the workspace
members. The separate first-party `scripts/lib/slice-rs` helper crate and the
vendored, locally-patched `src-tauri/vendor/compass_data` path-dependency are
**not** gated (vendored code is left as-is on purpose; `compass_data` is also
explicitly kept out of the workspace via `[workspace] exclude`, so
`--workspace` commands never touch it) - don't assume `cargo fmt` touched
them. The third standalone crate, the agent harness's macOS-only OS helper
(`e2e/agent/os/helper`), is gated by its own path-filtered workflow
(`.github/workflows/os-helper.yml`: `cargo fmt --check`, `cargo clippy -- -D
warnings` with clippy's default set, its tests and a release build, on a macOS
runner) rather than by this policy.

The crate was adopted into rustfmt in one mechanical commit (recorded in
`.git-blame-ignore-revs`, so `git blame` skips it - wire it locally once with
`git config blame.ignoreRevsFile .git-blame-ignore-revs`). If a `cargo fmt`
diff ever looks large, that's normal re-wrapping; the gate keeps it from
drifting again.

## Why this exists: the rot lesson

The strict policy was added in **Feb 2026 (`17dc4feb`)**, explicitly to
guardrail AI-written Rust - its comments call out the patterns directly: *"AI loves
.unwrap()"*, *"Clone-spam - AI appeases the borrow checker with .clone()"*, *"String
nonsense AI generates"*, *"Prevent AI from silencing lints"*.

But `[lints.clippy]` **only takes effect under `cargo clippy`**, and CI never ran
clippy. So the policy silently **rotted**: by June 2026, ~4 months of new code had
reintroduced **172 deny-level violations**. The fix (PR #147/#149) was to restore
cleanliness *and* **gate clippy in CI** so it can't drift again.

> **Lesson: a lint policy that isn't a CI gate is decoration.** If you tighten the
> policy, make sure `check` still enforces it.

## The deny-level policy (the "must fix" set)

Production code may not use (see `[workspace.lints.clippy]` for the full list + comments):

- **Panic safety:** `unwrap_used`, `expect_used`, `panic`, `unwrap_in_result`
- **Debug artifacts:** `dbg_macro`, `todo`, `unimplemented`, `print_stdout`, `print_stderr`
- **Clone/string spam:** `inefficient_to_string`, `clone_on_ref_ptr`, `string_to_string`, `rc_clone_in_vec_init`
- **Hygiene:** `wildcard_imports`, `allow_attributes` (you can't silence lints with `#[allow]`), `wildcard_dependencies`, `redundant_type_annotations`, `mem_forget`, `lossy_float_literal`, `needless_pass_by_ref_mut`, `semicolon_if_nothing_returned`, `explicit_into_iter_loop`
- **Async footguns:** `await_holding_lock`, `await_holding_refcell_ref`
- **`[workspace.lints.rust]`:** `unsafe_code`, `unused_must_use`, `unused_qualifications`, `unreachable_pub`

`pedantic` is on at **warn** (advisory, with specific `allow`s for known-noisy
members). Only deny-level lints fail the build.

## Test code: the per-module `#[expect]` idiom

Test code legitimately uses `unwrap`/`expect`/`panic`/`print`. The **deliberate
convention** (not `clippy.toml`, not `#[allow]` - which is denied) is a
per-test-module `#[expect(...)]` listing exactly the lints that module uses:

```rust
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::float_cmp)]
mod tests;                                   // on the `mod tests;` declaration
```
or, for an inline module, directly above `mod tests {`. `#[expect]` (not
`#[allow]`) is required because it *also* warns if the lint stops firing, so the
exemptions don't go stale. When you add a test that uses a new denied lint
(e.g. a `println!` diagnostic), add that lint to the module's `#[expect]`.

> Do **not** introduce `clippy.toml` `allow-*-in-tests` - it conflicts with the
> ~90 existing test-module `#[expect]` attributes (it suppresses the lint,
> leaving them unfulfilled). Follow the existing idiom.

## Extending the policy: measure, don't vibe

When adding lints, **measure the hit count on the real tree first** - a
conceptually-good lint can be unusable in practice:

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --features telemetry --all-targets \
  --message-format=json -- -W clippy::<candidate> | \
  python3 -c "import json,sys; print(sum('clippy::<candidate>' in l for l in sys.stdin))"
```

Prefer **surgical** lints (catch a specific AI pattern, low false-positive) over
**blunt** ones, and **respect the existing `allow`s** - `too_many_lines` and
`too_many_arguments` are allowed *on purpose* for the geometry/numerical code.

### Considered and deliberately deferred

Recorded so they aren't naively re-proposed. Counts measured June 2026 on the
clean tree.

| Lint | Hits (prod/test) | Why not (yet) |
|------|------------------|---------------|
| `indexing_slicing` | 831 / 829 | Right instinct (`v[i]` panics, which `panic=deny` doesn't catch) but pervasive in legitimate code - denying = noise. Revisit as a focused audit, not a blanket gate. |
| `default_numeric_fallback` | 1206 / - | Flags every untyped numeric literal. Impractical. |
| `str_to_string` | 1002 / 1866 | `"x".to_string()` vs `.to_owned()` - stylistic, codebase-wide; `inefficient_to_string` already covers the perf case. |
| `string_slice` | 53 / - | Byte-slicing `&s[a..b]` can panic on non-char-boundary, but most uses here are ASCII-safe. Candidate for a future audit. |
| `let_underscore_must_use` | 42 / 10 | Many `let _ =` discards are intentional. |
| `map_err_ignore` | 32 / - | `.map_err(\|_\| …)` swallows errors (real AI tell), but discarding the source error is often deliberate. Warn-only candidate. |
| `get_unwrap` | 7 / 18 | `.get(i).unwrap()` is a panic door worth closing - deferred only for the test-module `#[expect]` churn (18 sites). Good next add. |
| `panic_in_result_fn` | 1 / - | The one hit is a *legitimate* invariant assert in the LSQ solver (`project.rs`). The lint conflates invariant-asserts with slop, so it'd just need a per-site `#[expect]`. |
| `unreachable`, `exit` | 3 / 1 | Fire only on `#[tauri::command]` / `.run()` **macro expansions**, not our code - adding them would force permanent framework `#[expect]`s for ~zero AI-slop value. |
| `too_many_arguments` (deny), `cognitive_complexity`, `too-many-lines-threshold` | - | Contradict deliberate `allow`s for the complex geometry/numerical code. |
| `transmute_ptr_to_ref`, `cast_ptr_alignment` | - | Redundant: `unsafe_code = "deny"` already blocks anything that needs `unsafe`. |
| `cargo_common_metadata`, `multiple_crate_versions` | 15 / 44 | Matter for *published* crates; this is a private app binary with a large transitive tree. |

## Frontend: ESLint

The TypeScript side has one linter gate, ESLint, added in #576 (D1). Before it
there was no JS/TS linter at all, yet the tree carried ~40 `eslint-disable`
comments for rules that had never run - the same "policy that isn't a gate"
rot as the clippy story above.

| Piece | Where |
|-------|-------|
| Config | `eslint.config.js` (flat config) |
| Gate | `pnpm lint:eslint` (`scripts/check-eslint.mjs`), fixture test `pnpm lint:eslint:test` |
| Warning baseline | `eslint-baseline.json` (generated: `pnpm lint:eslint --update`) |
| CI | `check` and `smoke` jobs, step "ESLint (hook rules, ratcheted warnings)" - the same full-scope run in both, because `check` path-ignores `e2e-playwright/**` and `smoke` path-ignores `e2e/agent/**` |

**Scope and rules.** `src/**/*.{ts,tsx}` (tests included; the generated IPC
types are ignored) and every `.ts` file under the two test-automation trees,
`e2e-playwright/` and `e2e/`. Parsed with typescript-eslint's parser and **no
type-aware rules** (no `parserOptions.project`, so a run needs no TypeScript
program).

- **typescript-eslint `recommended`** (not `recommended-type-checked`),
  everywhere. `no-unused-vars` exempts `_`-prefixed names, the convention tsc's
  `noUnusedParameters` already imposes on the tree.
- **`react-hooks/rules-of-hooks`** - error, `src/` only (nothing else renders
  React; a Playwright fixture's `use()` would read as a hook). A hook behind
  `||`, a condition or an early return runs on some renders and not others,
  and React throws "Rendered more hooks than during the previous render". The
  first run found one (`PlacementOverlay`, a store hook after an early return).
- **`react-hooks/exhaustive-deps`** - warn, ratcheted, `src/` only. A missing
  effect/memo dependency can be a stale-closure bug, but the tree carried ~100
  findings when the rule landed, so they are frozen rather than fixed in one
  sweep.

Severity is per tree. In `src/` every recommended rule is an error (the set
found three findings there, fixed when it landed). In `e2e-playwright/` and
`e2e/`, `no-unused-vars` is a ratcheted warning: the set found 236 `any` across
22 Playwright specs and 25 unused names, which is test debt to pay down file by
file, not a reason to keep the gate off those trees. The `any`s are paid down,
so `no-explicit-any` is an error there too (#576 NF-W7F-5).

- **`stalagmight/no-render-ref-write`** - error, `src/` only; the repo's own
  rule (`scripts/eslint/`, registered in `eslint.config.js` as the
  `stalagmight` plugin). No `ref.current = ...` while a component or hook
  renders: a render React throws away (an error-boundary retry, a suspended
  or interrupted transition) would leave its value behind. Mirror a latest
  value with `useLatest(value)` (`src/hooks/useLatest.ts`, which publishes it
  in a layout effect), or write the ref in an effect or handler. React's
  documented lazy init (`if (ref.current === null) ref.current = ...`, `??=`)
  is allowed. Added in #576 (D17), which migrated the 42 writes it found.
  `exhaustive-deps` does not know a `useLatest` ref is stable, so it asks for
  it in dependency arrays; listing it is harmless (the object never changes).
- **`stalagmight/require-store-selector`** - error, `src/` (tests included),
  zero tolerance. A zustand store hook called without a selector
  (`useProjectStore()`) subscribes its component to the whole store, so every
  `set()` re-renders it: a dirty-flag flip, a loading tick, the undo
  availability after each edit. Also flagged: an identity selector
  (`(s) => s`, bare or under `useShallow`), `useStore(store)` with no
  selector, a selector that builds a new object or array with no `useShallow`
  and no equality function (`(s) => ({ a: s.a })`, `(s) => [s.a]`,
  `(s) => s.list.map(f)`: zustand compares answers with `Object.is`, so a fresh
  one on every call re-renders on every write), and `useShallow((s) => ({ ...s }))`
  (#576 RV5-LS-3). A fallback such as `(s) => s.list ?? []` is not flagged: it
  is the stored list whenever there is one. Select the fields the component reads -
  `useProjectStore((s) => s.project)`, or
  `useShallow((s) => ({ a: s.a, b: s.b }))` for several. Store actions are
  stable, so selecting them costs nothing; a getter action that reads state
  through `get()` while rendering (`isShotFrozen`) needs the state it reads
  picked too, or the component stops re-rendering when it changes. Added in
  #576 (D2), which converted the 87 calls it found (the app root, the map
  canvas and the side panels among them); the render-count proof is
  `src/hooks/__tests__/storeSelectorRenders.test.tsx`.

`linterOptions.reportUnusedDisableDirectives` is `error`, so a directive that
suppresses nothing, or names a rule that is not loaded (`react/...`,
`jsx-a11y/...`), fails the gate instead of rotting.

**The ratchet.** `eslint-baseline.json` records the warning count per rule and
per file. The gate fails when a count grows, when an unrecorded file or rule
warns, and - so the count can only go down - when a count drops below its row:
fixing a warning means running `pnpm lint:eslint --update` and committing the
smaller baseline. Errors are never baselined. Raising a number is a
review-visible decision; fix the warning unless there is a reason not to.
It is a count ratchet (#576 RV5-LS-5): it compares the number of warnings per
rule and file, not which lines warn, so fixing one warning and adding another
of the same rule in the same file passes unseen; and because `--update`
rewrites every row from the current tree, it also locks in any growth made
elsewhere in the same change - read the `eslint-baseline.json` diff in review,
not only the rows you meant to lower.

**Disabling a rule on one line** needs a reason after ` -- `, and
`stalagmight/require-disable-reason` (error, every linted tree; #576 RV5-LS-4)
fails an `eslint-disable`, `-line` or `-next-line` directive without one. A
bare `/* eslint-disable */` turns that rule off with every other, so it is the
one shape left to review:

```ts
// eslint-disable-next-line react-hooks/exhaustive-deps -- mount-only: hash the file this dialog opened with, once
```

Every kept disable carries one, including the three that
`scripts/generate-replay.mjs` writes into generated replay specs. The review
that added the gate deleted the directives that suppressed nothing or named
rules that are not loaded (`react/...`, `jsx-a11y/...`, and core rules such as
`no-new-func` that no config enables), and fixed the ones whose missing
dependency was a stable identity (a `useCallback(..., [])` setter, a callback
whose own inputs were already listed).

**Extending it.** Same rule as for clippy: measure on the real tree first
(`pnpm exec eslint src e2e-playwright e2e --rule '{"<rule>": "warn"}' -f json`), prefer surgical
rules, and land a new warn-level rule with its counts in the baseline rather
than with a sweep of unrelated fixes.

### Import cycles

`pnpm lint:import-cycles` (`scripts/check-import-cycles.mjs`, fixtures in
`pnpm lint:import-cycles:test`, CI `check` job) fails on any value-import
cycle among the `.ts`/`.tsx` modules under `src/`, tests included. Zero
tolerance, no baseline: the tree had one cycle when the gate landed (#576
D27-32(b): projectStore, speleoStore and three modules pulled into the same
loop), and it was broken first.

Why it matters: two modules that import each other's values evaluate in the
order some third module happened to import them, and the second one sees the
first's exports before they exist. A single top-level read of a cross-import
then crashes the app at start, for one import order only.

What counts as an edge: default, named, namespace and side-effect imports,
`export ... from` and `export * from`. Not an edge: `import type`,
`export type`, an import whose every specifier is an inline `type`, and a
dynamic `import()` (it runs after both modules evaluated). To break a loop,
move the shared value to a leaf module (`src/store/commandResult.ts`), pass it
in as a parameter (`surfaceSaveFailure` takes the integrity state), or
late-bind it (`src/store/speleoStoreHandle.ts`); when only types cross, use
`import type`.

### Dialog i18n literals

`pnpm lint:i18n-literals` (`scripts/check-i18n-literals.mjs`, fixtures in
`pnpm lint:i18n-literals:test`, CI `check` job) fails on hard-coded
user-visible text in the dialog layer: every production file under
`src/components/Dialogs/` and each `src/components/common/` module a dialog
imports (Button, Select, Toggle, ...; derived from the imports on every run),
plus the shared UI with text of its own that no dialog imports, pinned in the
script's `PINNED_COMMON` (the survey context menu, the panel error fallback and
the entity link, #576 NF-W6-2). A pinned file that no longer exists fails the
gate, so a rename cannot quietly drop it from scope.
Zero tolerance (#576 D20): the dialogs carried ~470 such strings, including
the Error / Confirm primitives, the unsaved-changes prompt every dialog frame
shows and 13 validation errors, and they were moved to
`src/i18n/locales/en.json` with the English unchanged.

What it reads as UI text (TypeScript AST, syntax only): JSX text, strings in a
JSX child expression (followed through `?:`, `&&`, `??`, `||`, `+`), the
user-facing JSX attributes (`title`, `label`, `placeholder`, `alt`,
`aria-label`, ...), the same names as object-literal properties (dialog
payloads, option lists), the argument of a `set...Error` /
`...Message` / `...Warning` / `...Notice` setter, a `showToast` message, the
argument a message-named parameter (`message`, `reason`, `...Text`) of a
function declared in the same file takes (`fail(kind, message)`; not when every
use of that parameter is an argument of `console.*`), and every string held by a
binding named as text (`FALLBACK_TEXT`, `emptyMessage`, `STEP_LABELS`; i18n keys
and lowercase code tokens such as `'timeout'` or `'section-'` excepted, #576
RV6a-3 - a token is still caught where the file renders it). It follows the file's own `const`/`let`
bindings into those places (`{HEADING}`, a template held in `msg`,
`{NAMES[kind]}`) and through `String(...)` (#576 RV5-LS-1, RV5-LS-2). It never
looks inside any other call, so `t('key')` is invisible to it, and a string with no letter left once
`${...}` placeholders and HTML entities are removed passes (`{' · '}`,
`&rarr;`). Render the text with `t('dialogs.<dialog>.<key>')`, or `<Trans>`
with named tags when it wraps elements (see `OverwriteConfirmDialog.tsx`).

A literal that is not language - a file-format name (PNG, PDF), a brand, a
URL placeholder - goes in `scripts/i18n-literals-allowlist.json` as an exact
`{file, text, reason}` row. A row that matches nothing fails the gate, so the
list cannot outlive its reason. The gate is a heuristic: bindings resolve by
name within one file, so a string that arrives from another module, a prop or
a helper's return value is invisible to it, which is what review is for.

### vitest: the shared utils/ipc mock

`pnpm lint:vitest-ipc-mock` (`scripts/check-vitest-ipc-mock.mjs`, fixtures in
`pnpm lint:vitest-ipc-mock:test`, CI `check` job) fails on any `vi.mock` or
`vi.doMock` of `src/utils/ipc` that passes a factory: the copied
`vi.hoisted(() => vi.fn())` + `vi.mock('<rel>/utils/ipc', () => ({ invokeIpc,
getErrorMessage }))` idiom, an `async` or `function` factory, a factory held in
`vi.hoisted`, or an `() => import(...)` module. Zero tolerance, no baseline:
#576 E15 replaced about 120 such copies with one manual mock, and six new ones
had appeared within a wave when the gate landed (E15-NF2); they were moved to
the shared mock first.

Use the shared mock instead - `vi.mock('<rel>/utils/ipc')` with no factory,
scripted through `invokeIpcMock` and `stubGetErrorMessage` from
`src/test/factories/ipc.ts` (docs/testing-recipes.md, "Factories and the
shared IPC mock"). The specifier is resolved from the file, so `'../ipc'` in
`src/utils/__tests__/` counts, with or without an extension, root-relative
(`'/src/utils/ipc'`) or in the typed `vi.mock(import('...'))` form; a mock of
any other module does not. Comments and strings are skipped.

A file that genuinely needs its own factory sits in the script's `ALLOWLIST`
with its reason: today `useInternalErrorEvent.pull.test.tsx` (its
`invokeIpc` must not be a `vi.fn`) and the `vi.resetModules()` pair
(`storeLoadOrder.test.ts` and the factory test that pins its idiom). A row
whose file is gone or has no factory left fails the gate, so the list cannot
outlive its reason.

## TypeScript: one shared base

Every TypeScript program at the repo root extends `tsconfig.base.json`
(#576 E11). Before it, the three configs restated their options with no
`extends` and had drifted: the two test-automation trees, about 100k lines,
were type-checked without six of the app's strictness flags.

| Program | Config | Gate | Checks |
|---------|--------|------|--------|
| App | `tsconfig.json` | `pnpm tsc --noEmit` (CI `check`) | `src/` |
| Playwright | `tsconfig.playwright.json` | `pnpm typecheck:playwright` (CI `smoke`) | `e2e-playwright/`, `playwright.config.ts`, and the `src/` files the specs import |
| Agent harness | `tsconfig.agent.json` | `pnpm typecheck:agent` (CI `check`) | `e2e/agent/`, `e2e/lib/` (not the generated `e2e/agent/replays/`, #466) |

**What lives where.** The base holds everything the programs share: the module
settings (`ESNext`, `bundler`, `resolveJsonModule`, `isolatedModules`),
`skipLibCheck`, `noEmit`, and all of the strictness - `strict` with its family
spelled out, `noImplicitReturns`, `noFallthroughCasesInSwitch`,
`noUnusedLocals`, `noUnusedParameters`, `exactOptionalPropertyTypes` and
`noUncheckedIndexedAccess`. A program's own file keeps only what genuinely
differs: `target`/`lib` (ES2020 for the app, ES2022 for the e2e trees), `jsx`,
`types`, the interop flags the e2e trees need, `include`, and its own
incremental cache (`node_modules/.cache/tsc/<config>.tsbuildinfo`). Do not
set a strictness option in a program file: change it in the base, for every
program at once, and fix what it finds in all three trees.

**Fixing what the two strict index/optional flags find in test code.** Parity
cost 25 unused-binding fixes, 34 `exactOptionalPropertyTypes` fixes and 286
`noUncheckedIndexedAccess` fixes, all type-level: no spec or harness step
behaves differently. The same rules apply to new code:

- `noUncheckedIndexedAccess`: first a type that says what is already true - a
  helper that waits for at least one item returns a non-empty tuple
  (`waitForIpcCall`), a setup that throws unless it made N sections is
  overloaded per N (reverse-section's `setup()`). Otherwise a non-null `!`
  where the index is provably in range (a length assertion or a poll just
  above, a loop bound, `arr.length ? arr[arr.length - 1] : ...`), with a short
  reason when the proof is not on the same line; or an explicit `throw` where
  the unguarded access would already have thrown (not where it carried on, as
  a comparison, a spread or a value passed along does). Never a fallback such as
  `?? 0`, `|| ''`, `?? []` or a new `?.`: in a test, a fallback turns a
  failure into a pass.
- `exactOptionalPropertyTypes`: when callers already pass `undefined` and the
  receiver reads it as absent (`??`, `!== undefined`, JSON that drops it),
  say so in the type, `prop?: T | undefined`, rather than reshaping the
  values.

**Copied, not extended.** `telemetry-worker/tsconfig.json` does not extend the
base: the worker is a standalone pnpm project with its own install, and
`.github/workflows/telemetry-worker.yml` re-runs only on `telemetry-worker/**`
changes, so a root file it depended on could change its result unseen. It
carries a copy of the base's strictness flags instead (#576 NF-W9E11-1): a flag
added to or dropped from the base is changed there too, by hand.

**Not covered.** `vite.config.ts`, `scripts/**/*.ts` and `src-tauri/bindings/` are in no
program at all (see [Testing recipes: Typecheck](testing-recipes.md#typecheck)).
