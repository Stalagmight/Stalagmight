# Stalagmight

Tauri desktop app for processing underwater cave survey data (Rust backend + React/TypeScript frontend).

It is year 2026. No em-dashes: write an ASCII hyphen instead, instead of '—' use '-'. This covers the
hyphen, non-breaking hyphen, figure dash, en dash, em dash, horizontal bar,
minus sign, soft hyphen, the small, fullwidth, two-em, three-em and vertical
dash forms, the double hyphens and the Armenian and oblique hyphens
(U+2010..U+2015, U+2212, U+00AD, U+FE58, U+FE63, U+FF0D, U+2E3A, U+2E3B,
U+FE31, U+FE32, U+2E40, U+2E17, U+058A, U+2E5D). The
soft hyphen is invisible inside a word, so delete it rather than replace it.
Box-drawing characters in ASCII diagrams are fine.

## Quick Reference

| Task | Command |
|------|---------|
| Install deps | `pnpm install` |
| Dev server | `./scripts/dev.sh` |
| Type check | `pnpm tsc --noEmit` |
| Playwright tests | `pnpm test:ui` |
| Rust tests (Linux/macOS) | `TELEMETRY_ENDPOINT=http://localhost/dummy cargo test --manifest-path src-tauri/Cargo.toml --workspace --features telemetry` |
| Rust tests (Windows) | `./scripts/test.ps1` |
| Production build | `pnpm tauri build` |

## Critical Rules

### Pinned Toolchain - enable corepack

pnpm `11.18.0` (via `"packageManager"` in `package.json`) and node `24.x` (via
`.nvmrc`) are pinned so every machine + CI runs the same versions. First-time
setup on a clean clone:

```bash
corepack enable      # makes `pnpm` honor packageManager (pins to 11.18.0)
pnpm install         # build scripts are pre-approved (pnpm-workspace.yaml)
./scripts/dev.sh
```

Rules that keep the pin working (full detail in [docs/toolchain.md](docs/toolchain.md)):

- Bumping pnpm means regenerating the corepack integrity hash in
  `packageManager` - a stale or npm-format hash makes corepack refuse to run.
- Build-script approvals live in `pnpm-workspace.yaml` as an `allowBuilds:` map
  keyed by package (`esbuild: true`), never a list - other shapes fail on
  pnpm 11 or are silently ignored on pnpm 10.
- `pnpm-lock.yaml` stays at `lockfileVersion: '9.0'`; don't commit
  lockfile-only drift.

### Never Use `cd` in Bash Commands

The shell session persists working directory changes, causing commands to run from wrong locations. This has caused files to be created in `src-tauri/src-tauri/` and committed to git.

```bash
# BAD - persists directory change
cd src-tauri && cargo check

# GOOD - use manifest path
cargo check --manifest-path src-tauri/Cargo.toml

# GOOD - subshell isolates change
(cd src-tauri && cargo check)
```

### Pre-Push Checklist

All must pass before pushing:

```bash
# --- check job (ci.yml) - same order as CI: lints first, then the gates and tests ---
cargo fmt --manifest-path src-tauri/Cargo.toml --check    # root package (run without --check to fix)
for m in src-tauri/crates/*/Cargo.toml; do cargo fmt --manifest-path "$m" --check; done   # every member (CI runs this loop, NOT --all - see docs/linting.md)
TELEMETRY_ENDPOINT=http://localhost/dummy cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --features telemetry --all-targets   # deny-level lint gate
TELEMETRY_ENDPOINT=http://localhost/dummy cargo check --release --manifest-path src-tauri/Cargo.toml --workspace --features telemetry   # release profile: debug_assertions off (#541)
pnpm tsc --noEmit
pnpm lint:test-ids:test && pnpm lint:test-ids             # interactive UI test-ID contract
pnpm lint:eslint:test && pnpm lint:eslint                 # ESLint over src + e2e trees, warnings ratcheted (eslint-baseline.json); smoke runs it too
pnpm lint:import-cycles:test && pnpm lint:import-cycles   # no value-import cycle under src/ (import type is fine)
pnpm lint:i18n-literals:test && pnpm lint:i18n-literals   # no hard-coded UI text in the dialogs (strings come from en.json)
pnpm lint:vitest-ipc-mock:test && pnpm lint:vitest-ipc-mock   # no vi.mock of utils/ipc with a factory outside the allowlist (use the shared mock)
pnpm typecheck:agent                                      # agent-harness typecheck
./scripts/test-release.sh                                 # release tooling, driven in throwaway clones
pnpm lint:rust-file-size:test && pnpm lint:rust-file-size # Rust file-size ratchet
pnpm lint:ts-file-size:test && pnpm lint:ts-file-size     # TS file-size ratchet
pnpm lint:readonly-gates:test && pnpm lint:readonly-gates   # read-only gating tripwire
pnpm exec vitest run                                      # vitest unit + component (plain "pnpm test" watches and never exits locally)
TELEMETRY_ENDPOINT=http://localhost/dummy cargo test --manifest-path src-tauri/Cargo.toml --workspace --features telemetry
git status --porcelain src-tauri/bindings                 # must print nothing after cargo test (generated ts-rs bindings are committed)
pnpm gen:ipc-types                                        # regenerate the frontend IPC types from the Rust types
git status --porcelain src/types/ipc src/types/ipc.generated.ts   # must print nothing after gen:ipc-types (the generated frontend IPC types are committed)
cargo test --locked --manifest-path src-tauri/vendor/compass_data/Cargo.toml   # workspace-excluded vendored crate; no telemetry feature/env
cargo package --no-verify --manifest-path src-tauri/Cargo.toml -p stalagmight-models -p stalagmight-persistence -p stalx-reader   # crates.io packaging check; each package must also list LICENSE
STALAGMIGHT_EXPECT_UNSTRIPPED_SQLITE=1 CARGO_TARGET_DIR=target-unstripped-sqlite cargo --config 'env.LIBSQLITE3_FLAGS.value=""' --config 'env.LIBSQLITE3_FLAGS.force=true' test --manifest-path src-tauri/Cargo.toml -p stalagmight-persistence -p stalx-reader --lib -- --exact persistence::sqlite::untrusted_sql_tests::every_untrusted_open_refuses_what_the_build_flags_strip tests::the_inspect_connection_refuses_what_the_build_flags_strip   # #619: the SQLite authorizer on a build WITHOUT the repo flags (what crates.io users get)
cargo +1.89.0 check --locked --manifest-path src-tauri/Cargo.toml -p stalx-reader -p stalagmight-persistence -p stalagmight-models   # MSRV of the published crates (needs: rustup toolchain install 1.89.0 --profile minimal; #625)
# --- smoke job (e2e-smoke.yml) ---
pnpm lint:e2e-parallel:test && pnpm lint:e2e-parallel     # Playwright parallel-safety
pnpm lint:e2e-no-sleep:test && pnpm lint:e2e-no-sleep     # no unannotated waitForTimeout in Playwright specs
pnpm typecheck:playwright                                 # e2e-playwright specs + IPC mock
pnpm test:mock-contract                                   # IPC mock <-> generate_handler! contract
pnpm lint:smoke-features:test && pnpm lint:smoke-features # every critical feature in docs/TESTING.md has an @smoke test
pnpm test:pw-timing                                       # timing reporter self-test (a report, not a time budget)
pnpm test:ui:smoke                                        # the @smoke merge gate (needs a display, e.g. xvfb-run)
# plus the specs for the feature you changed: pnpm exec playwright test e2e-playwright/<feature>.spec.ts
# --- leak-scan job (leak-scan.yml; runs on every change, docs included) ---
pnpm lint:private-leaks:test && pnpm lint:private-leaks   # no internal/personal refs in the public tree
pnpm lint:no-em-dash:test && pnpm lint:no-em-dash         # Unicode-dash guard
pnpm lint:third-party:test && pnpm lint:third-party       # THIRD-PARTY-LICENSES.md matches the lockfiles (regen: pnpm gen:third-party)
pnpm lint:updater-pubkey:test && pnpm lint:updater-pubkey # the committed updater pubkey and endpoints are well-formed (#579)
pnpm gen:updater-manifest:test                            # updater manifest generator fixture test (#579)
pnpm lint:prepush-checklist:test && pnpm lint:prepush-checklist # this block lists every gate CI runs
```

The full `pnpm test:ui` suite is deliberately not on the list: it is heavy and
runs nightly. Run the `@smoke` set (the CI merge gate) plus the Playwright specs
for the code and functionality the change affects, selected by hand.

The block is the floor of what CI runs, not a summary of it:
`pnpm lint:prepush-checklist` (leak-scan) fails when `ci.yml`, `e2e-smoke.yml`
or `leak-scan.yml` runs a gate this block lacks, so a new CI step lands with its
line here.

Rust tests must run with `--workspace`, `--features telemetry` and
`TELEMETRY_ENDPOINT` set, exactly as above - without `--workspace` only the
app crate runs, and a featureless `cargo test` skips the telemetry collector's
own tests (the registry-coverage tests run either way), so either omission can
pass locally while CI fails. For fast iteration on one
crate use `-p`, e.g.
`cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib fuzz_undo`
(`--features telemetry` only applies to the packages that define the feature:
`stalagmight`, `stalagmight-telemetry`). Run cargo from inside the clone: the
bundled-SQLite build flags live in the repo's `.cargo/config.toml`, which cargo
reads from its working directory, not from `--manifest-path`, and the
persistence tests fail without them ([docs/toolchain.md](docs/toolchain.md)). On
**Windows**, use `./scripts/test.ps1` instead (a bare `cargo test` aborts at
startup with `STATUS_ENTRYPOINT_NOT_FOUND`). All explained in
[docs/testing-recipes.md](docs/testing-recipes.md).

### Pre-PR Review - required before every PR

Never open a PR (`gh pr create`, push-to-open, or marking ready) until the repository
pre-PR review has run at least once on the current change: `/pre-pr` in Claude Code or
`$pre-pr` in Codex. If it hasn't run, run it first and decline any request to skip it.
If the user states that the pre-PR review has already run, take them at their word and
proceed.

Never open a PR in which either the current state or history contains fixture files 
(*.stalx, *.tml, *.tmlu, *.mak, *.dat, *.wpj, *.srv, *.dmp, *.gpx, *.csv,...) even if 
not including them breaks tests or behaviour. User MUST commit and PR/push fixtures 
deliberately and manually. LLM-driven push/PR of these fixtures is not allowed.

### Rust lints + formatting are CI-gated

The `check` job enforces **both** `cargo fmt --check` (root package + every
workspace member, via the loop above) and `cargo clippy --workspace`
(deny-level) on `src-tauri`; run `cargo fmt` and fix clippy findings before
pushing. The full policy - the strict anti-AI-slop clippy config
(`[workspace.lints]` in `src-tauri/Cargo.toml`, inherited by every member),
the per-test-module `#[expect]` idiom (no `#[allow]`, no `clippy.toml`), the
rustfmt scope (the vendored `compass_data` and `scripts/lib/slice-rs` crates
are not gated), and which lints were deliberately deferred - is in
[docs/linting.md](docs/linting.md).

### Playwright Tests Require a Headless Environment

Only run `pnpm test:ui` when a display server is available or in a headless environment (e.g. `xvfb-run`). Do **not** run Playwright tests in a bare terminal without a display - they will fail.

### Dev Server

Always use `./scripts/dev.sh` to restart the dev server (handles process cleanup).

### IPC Types Are Generated

TypeScript IPC types are generated from Rust via `ts-rs`. Do **not** hand-edit
or create files under `src/types/ipc/`, `src/types/ipc.generated.ts`, or
`src-tauri/bindings/`. The first two are regenerated by `pnpm gen:ipc-types`;
`src-tauri/bindings/` falls out of `cargo test --workspace` (every member
crate exports there via `#[ts(export, export_to = "../../bindings/")]`; the
app crate must not host bindings-defaulted exports) and is committed, so CI
fails if the tests leave it dirty - run the generator or the tests and commit
what they write. See [docs/ipc-typegen.md](docs/ipc-typegen.md) for the full
picture, including the one drift shape the gate cannot see.

### Commit Early, Commit Often

Make small, frequent commits as you work. Don't accumulate large uncommitted changesets.
Do not wait for user to prompt for commits, make commits as you go.

### Never commit or extract fixtures and limit their use
The following rules exist to prevent leaking private and propriatary data.

Commiting fixtures (*.stalx, *.tml, *.tmlu, *.mak, *.dat, *.wpj, *.srv, *.dmp, *.gpx, 
*.csv, ...) is strictly prohibitted even with users explicit consent. User can and 
should commit and push them manually if they wish to include them in the public repo.

Fixtures outside of this repository may be used for testing purposes with users explicit
consent. These tests should be marked skip and only run when the required fixtures are
available through the external paths.

When constructing tests (for example to prevent regressions), never extract parts of 
external fixtures. You can create fully synthetic data that reporoduces the issue such
that no data is leaked.

### Changelog

Update `CHANGELOG.md` every time a new version is tagged.

## Documentation

| Document | Contents |
|----------|----------|
| [Docs index](docs/README.md) | Every page under `docs/`, grouped, with a contributor reading order |
| [SPEC.md](docs/SPEC.md) | **Full specification** - data model, UI design, workflows, file formats, validation rules |
| [Testing Guide](docs/TESTING.md) | Which tier a test belongs in, and the UI testability requirements |
| [Testing Recipes](docs/testing-recipes.md) | Commands per tier, Playwright/agent-harness/Rust/vitest patterns, mock conventions, troubleshooting |
| [Linting & Format Policy](docs/linting.md) | Rust clippy/rustfmt CI gates, the anti-AI-slop policy, the per-test-module `#[expect]` idiom, considered-but-deferred lints; the frontend ESLint gate and its warning ratchet |
| [Toolchain](docs/toolchain.md) | The pnpm/node pins: corepack integrity hash, `allowBuilds` build-script approvals, lockfile version policy |
| [Architecture](docs/architecture.md) | Workspace crate layout, state flow, command pattern, key files, Rust patterns |
| [Project Structure](docs/project-structure.md) | Directory tree, tech stack |
| [Configuration](docs/configuration.md) | Config file locations, options, logging |
| [Saving](docs/saving.md) | Save pipeline, auto-save, undo persistence, atomic writes |
| [Inclination convention](docs/inclination.md) | Pitch is +down everywhere; where each importer normalises, and the v12 migration |
| [SpeleoDB dev testing](docs/speleodb-dev-testing.md) | Dev box env vars, the superuser test account, env-gated self-seeding live tests, verified v2 API contract |
| [Roadmap](docs/roadmap.md) | Historical record of the v1.0 phases; live work is in the issue tracker |
| [Telemetry](docs/telemetry.md) | Event reference, source tracking, adding telemetry to new features |
| [IPC Type Generation](docs/ipc-typegen.md) | How IPC TS types are generated and updated |
| [Performance](docs/performance.md) | Algorithm choices, renderer cost model, profiling guide |
| [Debug Gates](docs/debug-gates.md) | Where dev-only logs / asserts / profile harnesses live, and how to turn them on |
| [macOS Build](docs/macos-build.md) | macOS build scripts and known issues |
| [Agent Harness](docs/agent-harness.md) | LLM-agent harness (MCP + webdriverio) to drive the real app: tools, oracles, stress scenarios, replay + repro reports. Runs on the repo-pinned Node. |
| [UAT ledger](docs/uat/ledger.md) | Acceptance tier: one line per real-app UAT run, the per-RC checklist, and the machine-readable records. |
| [App modes](docs/app-modes.md) | Survey/Plan/Export modes and the read-only contract every mutation path must check |
| [Loop closures](docs/closures.md) | Raw Data View vs Closures Computed View, the least-squares solve, co-location with the move tools |
| [Undo/Redo](docs/undo-redo.md) | Command-pattern undo: two-phase capture, `UndoData` variants, batch commands, persistence, lock ordering |
| [Move tools](docs/move-tools.md) | Move Station/Shot/Segment: scope, the shared drag-preview engine, Ctrl+drag panning, immovable stations |
| [Map renderer](docs/map-renderer.md) | The custom WebGL survey layers: tessellation, shaders, labels, incremental updates |
| [Gas planning](docs/gas-planning.md) | Plan mode's gas engine: gas-weighted reachability over the survey graph, turnaround and stage-drop points |
| [DMP import](docs/dmp-import.md) | The mNemo `.dmp` import wizard: phases, virtual connections, suggestion scoring, overlap handling |
| [Ariane import](docs/ariane-import.md) | Reading `.tml`/`.tmlu`: schema, topology reconstruction, unit conversion, azimuth semantics, limitations |
| [Ariane export](docs/ariane-export.md) | Writing `.tml`/`.tmlu`: data mapping, topology, color conversion, station-comment co-encoding |
| [Releasing](docs/releasing.md) | Changelog -> tag -> release: `scripts/release.sh`, the CI workflows, where version numbers live |
| [UI style guide](docs/style-guide.md) | Design tokens, typography, buttons, panels, tables, forms, accessibility |
| [Notifications](docs/notifications.md) | Which surface (toast, banner, dialog, log) an event gets: decision tree, severity, dedup keys, copy rules |
| [macOS pan/zoom lockup](docs/debugging-macos-pan-zoom.md) | Postmortem of the macOS UI lockup, and how to re-enable the pan/zoom debug toolkit |
| [Touch support](docs/touch-support.md) | What touch/pointer support exists, the long-press paths, and the verified gaps |
