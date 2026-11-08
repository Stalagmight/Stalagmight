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
DBSTAT and extension loading (#608). The switch is `LIBSQLITE3_FLAGS`, which
libsqlite3-sys reads when it compiles the SQLite amalgamation, and its only
copy is the `[env]` table of the committed repo-root `.cargo/config.toml`.
It is set with `force = true`, so a stray value in your shell cannot weaken it;
to change the flags, edit that file.

Cargo finds `.cargo/config.toml` from its **working directory** upward, not
from `--manifest-path`. Every documented entry point runs cargo inside the
clone, so all of them get the flags: CI, the pre-push checklist and
`scripts/test.ps1` run from the repo root with `--manifest-path
src-tauri/Cargo.toml`, and `pnpm tauri build` / `dev` / `android build` run
cargo inside `src-tauri`. A cargo command started outside the clone (say
`cargo test --manifest-path ~/src/Stalagmight/src-tauri/Cargo.toml` from your
home directory) does not see the file and builds SQLite with every module.

The pinning test `bundled_sqlite_omits_virtual_table_modules_and_extension_loading`
(stalagmight-persistence) is not ignored: every `cargo test --workspace` runs it
and it fails on such a build. The release workflow also runs it on each
platform's build leg, because the strip works by a later `-U` cancelling
libsqlite3-sys's own `-D`, which each C compiler (MSVC on Windows) must honour.

The flags are defence in depth for the app build only. They do not travel with
a published crate, so a crates.io build of `stalagmight-persistence` or
`stalx-reader` has every module compiled in, and libsqlite3-sys has no feature
that leaves them out. What those builds rely on is a SQLite authorizer (#619):
every connection persistence opens on an existing file installs it before its
first statement (`deny_stripped_sqlite_features` in `schema_guard.rs`), as does
the connection `stalx-reader` opens itself to inspect a file, and it
refuses creating or dropping a virtual table, reading DBSTAT and the other
eponymous tables of those modules, and calling the functions FTS, R-tree and
extension loading register, whatever the build. Its tests,
`persistence::sqlite::untrusted_sql_tests::every_untrusted_open_refuses_what_the_build_flags_strip`
and stalx-reader's
`tests::the_inspect_connection_refuses_what_the_build_flags_strip`,
run in every `cargo test --workspace`, and the CI `check` job runs them once more
on a build with `LIBSQLITE3_FLAGS` emptied by `--config` (a command-line value
wins over `force = true`), in its own target dir, with
`STALAGMIGHT_EXPECT_UNSTRIPPED_SQLITE=1` so they fail unless the modules really
are there. That step also fails unless both tests ran and passed: an `--exact`
name that matches nothing runs no test and passes, which a rename would do.

Per-machine cargo settings (a target dir on another disk, a linker) do not
belong in the committed file. Use `$CARGO_HOME/config.toml`, the
`CARGO_TARGET_DIR` variable, or a `.cargo/config.toml` in a directory above the
clone: cargo merges every one it finds. A clone that already keeps an untracked
`.cargo/config.toml` of its own must move it aside before pulling the committed
one, or git refuses to overwrite it.
