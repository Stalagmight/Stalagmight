# Pinned Toolchain

The toolchain is pinned so every machine + CI runs the same versions. This page
is the full story behind the pins; the day-to-day rules live in the Pinned
Toolchain section of [`../CLAUDE.md`](../CLAUDE.md).

## The pins

- **pnpm `11.18.0`** via `"packageManager"` in `package.json` (corepack and
  `pnpm/action-setup` both read it - no per-workflow `version:`).
  `scripts/mac-setup.sh` reads the same field and installs that exact version.
- **node `24.x`** via `.nvmrc` + advisory `engines` (a warning, not a hard
  block, on newer node). One toolchain everywhere, the agent harness included.

## First-time setup on a clean clone

```bash
corepack enable      # makes `pnpm` honor packageManager (pins to 11.18.0)
pnpm install         # build scripts are pre-approved (pnpm-workspace.yaml)
./scripts/dev.sh
```

Prefer `corepack enable`: it is the only path that **verifies** the integrity
hash (a tampered tarball is refused outright). Any pnpm 10+ self-switches to
the pin before it does anything else, so a fresh clone installs correctly even
without `corepack enable`. A global pnpm 9 does not self-switch, and installs
only because it predates build approvals and runs the scripts anyway -
`corepack enable` is what puts it on the pin. Don't commit a lockfile-only
drift.

## The corepack integrity hash

The pin carries a corepack integrity hash (`pnpm@11.18.0+sha512.<hex>`), so
**bumping pnpm means regenerating it** - a stale hash makes corepack refuse to
run at all. The format is corepack's, **not** npm's: `+sha512.` + the tarball's
sha512 as **hex**. npm's registry `integrity` field is `sha512-` + **base64**
and corepack rejects it as an "Invalid package manager specification".
Regenerate with:

```bash
curl -sL https://registry.npmjs.org/pnpm/-/pnpm-<v>.tgz | shasum -a 512
```

## Build-script approvals: the `allowBuilds` shape is load-bearing

Approvals live in `pnpm-workspace.yaml`, as `allowBuilds:` keyed by package
(`esbuild: true`), never a list. The keyed map is the only form valid on both
pnpm majors (verified against the 10.34.5 and 11.18.0 release tarballs, one
isolated store per case):

- the older `pnpm.onlyBuiltDependencies` block in `package.json` is ignored
  from pnpm 11 on;
- a list under `allowBuilds` is rejected by both pnpm 10 and 11;
- the list form of `onlyBuiltDependencies` works on 10 but not 11.

On the pinned pnpm 11 getting it wrong is loud: it exits 1 with
`ERR_PNPM_IGNORED_BUILDS` naming the packages, so it cannot merge green. Do not
generalise that to pnpm 10, which **exits 0 and simply does not run the build
scripts**. Nothing in CI asserts the approval shape, so on any pnpm other than
the pin a wrong shape is silent - verify an install actually builds esbuild
rather than reading the exit code.

## Lockfile version

`pnpm-lock.yaml` stays at `lockfileVersion: '9.0'`; pnpm 11 accepts it under
`--frozen-lockfile` and does not rewrite it.

## rust-analyzer

A committed `rust-analyzer.toml` at the repo root configures rust-analyzer for
the `src-tauri` cargo workspace:

- `cargo.targetDir = true` - RA builds into its own target subdirectory instead
  of sharing the CLI target dir. Without this, RA (no features) and terminal
  runs (`--features telemetry`) evict each other's incremental caches, so every
  terminal test run triggered a full RA recheck.
- `cargo.features = ["telemetry"]` + `cargo.extraEnv.TELEMETRY_ENDPOINT` - the
  exact feature/env shape CI and the pre-push checklist use, so RA sees the
  telemetry module and flycheck matches CI. The env var is required: with the
  feature on, the telemetry member's build script
  (`src-tauri/crates/telemetry/build.rs`) refuses to build without it.

If your editor's rust-analyzer does not pick the file up (older RA builds only
read `rust-analyzer.toml` from certain roots), set the same three keys in your
editor settings, e.g. gitignored `.vscode/settings.json`:

```json
{
  "rust-analyzer.cargo.targetDir": true,
  "rust-analyzer.cargo.features": ["telemetry"],
  "rust-analyzer.cargo.extraEnv": { "TELEMETRY_ENDPOINT": "http://localhost/dummy" }
}
```

## Bundled SQLite build flags

A `.stalx` can come from anyone, so the bundled SQLite drops FTS3/4/5, RTREE,
DBSTAT and extension loading (#608) via `LIBSQLITE3_FLAGS`, which every
Rust-building workflow sets at the top level and the build scripts under
`scripts/` export. A plain local `cargo test` lacks it, so the pinning test
`bundled_sqlite_omits_virtual_table_modules_and_extension_loading` is ignored
locally and CI runs it with `--ignored`.
