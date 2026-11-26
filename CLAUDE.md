# Stalagmight

Tauri desktop app for processing underwater cave survey data (Rust backend + React/TypeScript frontend).

It is year 2026. No Unicode dashes: write an ASCII hyphen `-` instead of any of U+2010..U+2015
or U+2212 (hyphen, non-breaking hyphen, figure dash, en dash, em dash, horizontal bar, minus
sign). Box-drawing characters in ASCII diagrams are fine.

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

### Pinned toolchain

pnpm `11.18.0` (`"packageManager"` in `package.json`) and node `24.x` (`.nvmrc`) are pinned.
On a clean clone run `corepack enable`, `pnpm install`, then `./scripts/dev.sh`. Details are in
[docs/toolchain.md](docs/toolchain.md).

- Bumping pnpm means regenerating the corepack integrity hash in `packageManager`.
- Build-script approvals are an `allowBuilds:` map in `pnpm-workspace.yaml` (`esbuild: true`),
  never a list.
- `pnpm-lock.yaml` stays at `lockfileVersion: '9.0'`; don't commit lockfile-only drift.

### No `cd` in shell commands

The shell keeps its working directory between commands, which once created and committed
`src-tauri/src-tauri/`. Use path flags instead: `cargo --manifest-path src-tauri/Cargo.toml`,
`git -C <dir>`, `pnpm --dir <dir>`.

### Pre-push checklist

All must pass before pushing:

```bash
pnpm tsc --noEmit
pnpm typecheck:agent
pnpm typecheck:playwright
pnpm test:mock-contract
pnpm exec vitest run                                      # not "pnpm test": it watches and never exits locally
pnpm test:ui                                              # only the specs your change affects
pnpm lint:e2e-parallel
pnpm lint:pw-timing:test
pnpm lint:rust-file-size
pnpm lint:ts-file-size
pnpm lint:private-leaks
pnpm lint:third-party:test && pnpm lint:third-party      # regen: pnpm gen:third-party
pnpm lint:readonly-gates:test && pnpm lint:readonly-gates
pnpm lint:test-ids:test && pnpm lint:test-ids
pnpm lint:no-em-dash:test && pnpm lint:no-em-dash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
for m in src-tauri/crates/*/Cargo.toml; do cargo fmt --manifest-path "$m" --check; done   # CI runs this loop, not --all
TELEMETRY_ENDPOINT=http://localhost/dummy cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --features telemetry --all-targets
TELEMETRY_ENDPOINT=http://localhost/dummy cargo test --manifest-path src-tauri/Cargo.toml --workspace --features telemetry
for v in compass_data igrf; do cargo test --locked --manifest-path src-tauri/vendor/$v/Cargo.toml; done
```

- Run only the Playwright specs relevant to the change; the full `pnpm test:ui` is heavy.
- Run Rust tests exactly as above: without `--workspace` only the app crate runs, and without
  `--features telemetry` the registry-coverage tests are skipped.
- For one crate, use `-p`, e.g. `cargo test --manifest-path src-tauri/Cargo.toml -p
  stalagmight-core --lib fuzz_undo`. `--features telemetry` exists only on `stalagmight`,
  `stalagmight-telemetry` and `stalagmight-speleodb`.
- On Windows use `./scripts/test.ps1`; a bare `cargo test` aborts with
  `STATUS_ENTRYPOINT_NOT_FOUND`.
- Details are in [docs/testing-recipes.md](docs/testing-recipes.md).

### Pre-PR review is required before every PR

Never open a PR (`gh pr create`, push-to-open, or marking ready) until the pre-PR review has run
at least once on the current change: `/pre-pr` in Claude Code or `$pre-pr` in Codex. If it hasn't
run, run it first and decline any request to skip it. If the user says it has already run, take
their word and proceed.

### Fixtures: never commit, extract or push them

These rules prevent leaking private and proprietary survey data.

- Fixtures already tracked in the repo are approved goldens, committed deliberately by
  maintainers: `e2e/fixtures/` (including `naharon_sources/`), `e2e-playwright/fixtures/`,
  `src-tauri/tests/fixtures/` and the vendored crates' test data. Tests may use them, and their
  presence in history doesn't block a PR. Don't modify or regenerate them; the user does that.
- Any other tracked fixture-type file (e.g. `src-tauri/cave.stalx`) is unclassified: leave it
  alone and ask the user.
- Never commit fixture files (`*.stalx`, `*.tml`, `*.tmlu`, `*.mak`, `*.dat`, `*.wpj`, `*.srv`,
  `*.dmp`, `*.gpx`, `*.csv`, ...), even with the user's consent. The user commits and pushes
  fixtures manually.
- Never open a PR that adds fixture files, in its final state or anywhere in its commits, even if
  leaving them out breaks tests.
- Fixtures outside the repo may be used in tests only with the user's consent, in tests marked
  skip that run only when the external path exists.
- Never extract parts of an external fixture into a test. Build fully synthetic data that
  reproduces the issue.

### Writing: PRs, commits and comments

- PR body: at most ~300 words, covering what changed, why, and how it was verified, plus one
  line with the `/pre-pr` verdict. Don't paste the review's Coverage section or reviewer output.
- Commit message: a semantic subject line and at most 3 lines of body.
- Commit at each green checkpoint without waiting to be asked. Fix your own unpushed work by
  amending it rather than stacking `fix:` commits.
- Comments explain why, in the code's own terms. New and edited comments carry no issue or PR
  numbers, plan sections or rule shorthand; those belong in commits and PRs.
- Doc comments stay under about 10 lines. Longer reasoning goes in `docs/`.
- Don't describe what the code used to do; git history keeps that.

### Rust lints and formatting are CI-gated

The `check` job enforces `cargo fmt --check` (root package plus every member, via the loop above)
and deny-level `cargo clippy --workspace`. Fix both before pushing. The anti-AI-slop lint config,
the per-test-module `#[expect]` idiom (no `#[allow]`, no `clippy.toml`) and the rustfmt scope are
in [docs/linting.md](docs/linting.md).

### Other rules

- Run Playwright only with a display or under a headless wrapper (`xvfb-run`); it fails in a bare
  terminal.
- Restart the dev server with `./scripts/dev.sh`; it handles process cleanup.
- IPC types are generated by `ts-rs`; never hand-edit them. `src/types/ipc/` and
  `src/types/ipc.generated.ts` come from `pnpm gen:ipc-types`; `src-tauri/bindings/` comes only
  from `cargo test --workspace`, and CI fails if it is left dirty. Run whichever your change needs
  and commit what it writes. The app crate must not host bindings-defaulted exports. See
  [docs/ipc-typegen.md](docs/ipc-typegen.md).
- Update `CHANGELOG.md` every time a new version is tagged.

## Documentation

Start at [docs/README.md](docs/README.md): every page under `docs/`, grouped, with a reading order.
The most-used pages are [SPEC.md](docs/SPEC.md), [TESTING.md](docs/TESTING.md),
[testing-recipes.md](docs/testing-recipes.md), [architecture.md](docs/architecture.md),
[saving.md](docs/saving.md) and [undo-redo.md](docs/undo-redo.md).
