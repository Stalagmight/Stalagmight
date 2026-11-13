# Release Process

## Overview

Releases are created through Claude Code. The process:

1. Claude reviews git log since the last tag
2. Claude writes a human-readable summary into the `## [Unreleased]` section of `CHANGELOG.md`
3. Claude commits the changelog update
4. Claude runs `./scripts/release.sh <patch|minor|major> --no-edit --no-tag`
5. Claude tags with `./scripts/release-tag.sh vX.Y.Z[-rc.N]`, then pushes the
   commit and the tag
6. GitHub Actions builds the app and creates a release with artifacts

Three scripts, one each: `release.sh` bumps a version, `release-tag.sh` tags a
tree already at one, `release-notes.sh` renders what gets published.

⚠ **`--no-tag` at step 4 is what makes step 5 reachable.** Without it `release.sh`
tags `vX.Y.Z` itself, and `release-tag.sh` then refuses that tag as already
existing - correctly, but the advice it prints ("cut the next number") is wrong
for the situation. Dropping `--no-tag` is only right when you are cutting the
final release with no candidates and want the tag unchecked, which is the older
flow; prefer the two steps above.

## Release notes

**`CHANGELOG.md` is the release notes.** There is no second place to write them:
the workflow builds the published body from the changelog via
`scripts/release-notes.sh`. Write it for someone using the app, not for someone
reading the diff.

⚠ **`## [Unreleased]` is not automatically what gets published.** The script reads
the tag's own `## [x.y.z]` section and falls back to `[Unreleased]` only when that
section does not exist yet. Once `release.sh` has created it - which is the state
every release candidate after the first is cut from - anything written under
`[Unreleased]` ships inside the build and appears nowhere in its notes. This is not
hypothetical: four fixes went unannounced that way between `v0.5.0-rc1` and `rc2`.
**Fold post-candidate entries into the version's own section before tagging.**

**`release-tag.sh` refuses to tag when this is about to happen** - it renders the
body first and stops if `release-notes.sh` reports entries left behind, so the
warning now arrives while it can still be acted on. Pass `--allow-unannounced` to
ship them unmentioned deliberately.

The warning by itself is only advisory: the workflow runs `release-notes.sh`
after the tag is already pushed, where it reaches nothing but the CI log. A tag
created by hand therefore still gets no gate, which is the reason to go through
`release-tag.sh` rather than `git tag`.

Preview the exact body before tagging - it is the same script CI runs:

```bash
./scripts/release-notes.sh v0.5.2
./scripts/release-notes.sh v0.5.2-rc.1
```

The script picks the section, then appends the install instructions and the
checksum note that every release carries:

| Tag | Notes come from |
|-----|-----------------|
| `v0.5.2`, after `release.sh` moved the section | `## [0.5.2]` |
| `v0.5.2`, before that | `## [Unreleased]` (fallback) |
| `v0.5.2-rc.1` | `## [0.5.2]`, or `## [Unreleased]`, plus a candidate banner |
| anything with no matching section | `Release <tag>` (last resort) |

A candidate deliberately reuses the notes of the version it is a candidate for -
an RC *is* that release, built early - so the notes are written once.

## Release candidates

Bump once, then cut as many candidates as you like before the real thing:

```bash
./scripts/release.sh 0.5.2 --no-edit --no-tag      # bump + changelog, no tag
git push
./scripts/release-tag.sh v0.5.2-rc.1               # then rc.2, rc.3, ...
git push origin v0.5.2-rc.1
./scripts/release-tag.sh v0.5.2                    # the real one
git push origin v0.5.2
```

`--no-tag` matters: without it `release.sh` creates `v0.5.2` immediately, which
would either publish the real release early or sit on the wrong commit once
further fixes land before `rc.2`.

**Candidates are `vX.Y.Z-rc.N`, with the dot** (`v0.5.1-rc.1` was the first).
The rc number is then its own numeric identifier, so semver - and the updater -
orders `rc.10` above `rc.9`; the undotted `rc10` compares as text and sorts
below `rc9`. N counts from 1 with no leading zero: `rc.0` and `rc.01` are
refused, the latter because semver forbids a leading zero in a numeric
identifier and the updater would fail to parse it. The release workflow keys
on the exact shape too: only
`vX.Y.Z-rc.N` bakes the Beta channel and publishes to Beta (see [Updater
manifests](#updater-manifests)). `v0.5.0-rc1` to `-rc6` used the old shape;
`release-tag.sh` now refuses it and names the dotted spelling.

**Use `release-tag.sh` rather than a bare `git tag`.** The two scripts split the
work: `release.sh` owns a *new version* (bump the four files, move the changelog,
commit), `release-tag.sh` owns *tagging a tree already at that version* - which is
every candidate, and every release that follows one. A hand-typed `git tag` runs
none of the checks either script owns; that is how four fixes shipped unannounced
between `v0.5.0-rc1` and `rc2`
([#406](https://github.com/Stalagmight/Stalagmight/issues/406)). It refuses:

| Condition | Why |
|-----------|-----|
| the version files do not carry the tag's version | `v0.5.2-rc.1` cut from a `0.5.1` tree builds `Stalagmight_0.5.1_*` artifacts and an About box reading `0.5.1` |
| the working tree is dirty | a tag names a commit, so uncommitted notes are not the notes that publish |
| the tag already exists | candidates are kept, not recycled - a tester's bug report names one |
| the tag is not `vX.Y.Z` or `vX.Y.Z-rc.N` | those are the only shapes `release.yml` publishes; it asks the same `scripts/release-channel.sh` the workflow routes on. Another suffix (`-beta.1`) would publish a pre-release no Beta install is offered |
| the tag is the old undotted `-rcN` | the message names the `-rc.N` spelling to use instead |
| the tag is weekly-shaped | the weekly track is retired (see [The retired weekly track](#the-retired-weekly-track)) |
| the changelog has no notes for it | the published body would be the bare string `Release v0.5.0` |
| entries sit under `[Unreleased]` | they ship in the build and are named nowhere (`--allow-unannounced` to override) |

It also *notes*, without blocking, a final release that was never cut as a
candidate and a `## [X.Y.Z] - date` older than today - both legitimate, both
otherwise invisible.

It stops at a **local** tag and prints the push command. Pushing is the
irreversible step: it starts a paid three-platform build and, for a bare
`vX.Y.Z`, publishes as **Latest**. `--dry-run` does everything except create the
tag, which is the cheapest way to read the body you are about to publish.

- **The version files stay at the plain `0.5.2`.** The candidate lives only in
  the tag. This is not a style choice: the Windows MSI bundler rejects a
  non-numeric pre-release identifier, so `0.5.2-rc.1` in `tauri.conf.json` fails
  the Windows build outright. Artifacts are therefore named
  `Stalagmight_0.5.2_*` for every candidate.
- **CI bakes the pre-release part into the app instead.** For a `vX.Y.Z-rc.N`
  tag every build leg sets `STALAGMIGHT_PRERELEASE=rc.N` and
  `STALAGMIGHT_CHANNEL=beta` in the environment of `pnpm tauri build`; the app
  reads both at compile time. The first makes the app know it is `0.5.2-rc.1`,
  so the updater offers it `rc.2` and then the final; the second makes a fresh
  install from a candidate start on the Beta track. Neither is set for a final.
- **A tag with a suffix is published as a GitHub pre-release**, so it never
  becomes "Latest" and is not handed to people expecting the final build.
- **A candidate reaches Beta installs, never Stable ones.** It publishes
  `latest-beta.json` (and the weekly mirror); `latest-stable.json` changes only
  on a final.
- **About shows the full version**, so a candidate reads `0.5.2-rc.1` and cannot
  be mistaken for the final release. Keep candidates rather than deleting them -
  a tester's bug report names one.

## Step-by-step (what Claude does)

⚠ **First decide which of the two flows you are in. Running the wrong one cuts the
wrong version, and it commits before anyone sees it.**

```bash
# Is this version already bumped? Compare the version files to the release you mean.
grep -m1 '"version"' package.json
```

**Flow A - a NEW version** (`package.json` still shows the *previous* release):

```bash
# 1. Check what changed since last release
git log $(git describe --tags --abbrev=0)..HEAD --oneline --no-merges

# 2. Write summary into CHANGELOG.md under ## [Unreleased]
# (edit the file directly)

# 3. Commit the changelog
git add CHANGELOG.md
git commit -m "docs: update changelog for vX.Y.Z"

# 4. Bump all four version files, move the changelog, commit. `--no-tag` if any
#    candidate will be cut before the real release (see Release candidates above).
./scripts/release.sh <major|minor|patch|X.Y.Z> --no-edit

# 5. Push
git push origin master && git push origin --tags
```

**Flow B - the version is ALREADY bumped** (`package.json` already shows the version
you are releasing, because `release.sh … --no-tag` ran for an earlier candidate). This
is the flow for every candidate after the first, and for the final release that follows
them. **Do not run `release.sh` again** - it would bump you onto the *next* version:

```bash
# 1. Fold anything under ## [Unreleased] into that version's own section, and
#    commit. release-notes.sh reads `## [X.Y.Z]` and never looks at [Unreleased]
#    once it exists, so entries left behind ship in the build and appear in no
#    notes. release-tag.sh refuses the tag until this is done.

# 2. Render and READ the body that will actually be published, and check the
#    version files, the tree and the tag in the same pass.
./scripts/release-tag.sh vX.Y.Z-rc.N --dry-run    # or vX.Y.Z for the real one

# 3. Tag the commit you mean, then push the tag.
./scripts/release-tag.sh vX.Y.Z-rc.N
git push origin vX.Y.Z-rc.N
```

A candidate publishes as a pre-release and reaches Beta installs; a bare
`vX.Y.Z` publishes as **Latest** and reaches Stable and Beta installs.
Check the `## [X.Y.Z] - YYYY-MM-DD` date before the final tag: `release.sh` stamped it
the day the version was bumped, which may be well before the day it ships.
`release-tag.sh` says so when they differ, but only *you* can decide which date the
release should carry.

## What `scripts/release.sh` does

1. Reads current version from `package.json`
2. Computes new version (major/minor/patch or explicit X.Y.Z)
3. Verifies no uncommitted changes (except CHANGELOG.md)
4. Verifies `## [Unreleased]` section is non-empty
5. Updates version in all 4 files that carry it (see [Version locations](#version-locations)):
   - `package.json`
   - `src-tauri/tauri.conf.json`
   - `src-tauri/Cargo.toml`
   - `src-tauri/Cargo.lock` (this crate's stanza only)
6. Regenerates `THIRD-PARTY-LICENSES.md` with `pnpm gen:third-party`. The
   bundle stamps a hash of `src-tauri/Cargo.lock`, so a bump that left it
   behind turned leak-scan red on every release commit
   ([#648](https://github.com/Stalagmight/Stalagmight/issues/648)). This is the
   one step that needs a toolchain: pnpm, `cargo-about` 0.9.1, and a
   `node_modules` from a fresh `pnpm install --frozen-lockfile` (see
   `scripts/gen-third-party-licenses.mjs`). A missing `pnpm` refuses before
   anything is written; a failing generator reverts the version files and the
   bundle. `RELEASE_GEN_THIRD_PARTY` names another command to run instead
   (from the project root, no arguments), which is how `test-release.sh` stubs
   it
7. Moves `[Unreleased]` content under a new `## [X.Y.Z] - YYYY-MM-DD` header
8. Commits the four version files, `THIRD-PARTY-LICENSES.md` and `CHANGELOG.md`:
   `chore: release vX.Y.Z`
9. Tags: `vX.Y.Z`

### Flags

| Flag | Effect |
|------|--------|
| `--no-edit` | Skip opening `$EDITOR` on the changelog (required for Claude) |
| `--dry-run` | Print what would happen without making changes |
| `--no-tag` | Bump, move the changelog and commit, but create no tag - use when the next thing you cut is a candidate (`v0.5.2-rc.1`) rather than the release |

## What `scripts/release-tag.sh` does

Tags a tree that is *already* at the right version - see [Release
candidates](#release-candidates) for what it refuses and why. It writes nothing
but the tag: no version files, no changelog, no push.

1. Checks the tag's shape - `vX.Y.Z` or `vX.Y.Z-rc.N`, as `scripts/release-channel.sh`
   classifies it - and that it does not already exist
2. Checks the working tree is clean
3. Checks all 4 version files carry the tag's base version (`v0.5.2-rc.1` -> `0.5.2`)
4. Renders the release body with `release-notes.sh`, and refuses an empty one
5. Refuses if entries under `[Unreleased]` would go unpublished
6. Prints the body, then creates the annotated tag

### Flags

| Flag | Effect |
|------|--------|
| `--dry-run` | Every check, and print the body, but create no tag |
| `--allow-unannounced` | Tag anyway with entries left under `[Unreleased]` - they ship in the build, mentioned nowhere |

## CI/CD Workflows

### CI (`.github/workflows/ci.yml`)

- **Trigger**: push to master, pull requests to master
- **Steps**: cargo fmt --check, TypeScript checks (`pnpm tsc --noEmit` + `pnpm typecheck:agent`), test-ID / file-size / read-only lint gates, release-tooling tests, vitest, Rust tests, ts-rs bindings drift check, clippy, release-profile `cargo check --release`
- Installs: system libs, Rust toolchain, Node 24 (from `.nvmrc`), pnpm 11 (from `packageManager`)

### Release (`.github/workflows/release.yml`)

- **Trigger**: push of `v*` tags
- **Jobs**: `verify` -> `build` (3-platform matrix) -> `release`

| Job | Runner | Does |
|-----|--------|------|
| `verify` | `ubuntu-latest` | Classifies the tag with `scripts/release-channel.sh` (its candidate identity becomes the job's outputs), then TypeScript check + Rust tests + a release-profile `cargo check --release`, plus the third-party license bundle gate (leak-scan runs on branch/PR events only, so a tag push would otherwise skip it) - mostly a subset of the CI `check` job, not the whole gate. It does not run `cargo fmt --check`, clippy, vitest, the file-size ratchets or the private-leak/em-dash scans; those gate the merge, and a tag is cut from already-merged code |
| `build` | `ubuntu-latest`, `macos-latest`, `windows-latest` | `pnpm tauri build` (with `STALAGMIGHT_PRERELEASE` and `STALAGMIGHT_CHANNEL` exported for a candidate only), collects that platform's bundles, checksums them, uploads them as a job artifact |
| `release` | `ubuntu-latest` | Classifies the tag again, preflights the `updater` release, downloads all three artifact sets, re-verifies the checksums, asserts every platform is represented, extracts the changelog section, creates the GitHub release via the `gh` CLI, then publishes the updater manifests (see [Updater manifests](#updater-manifests)) |

- **Artifacts**: `.deb` / `.rpm` / `.AppImage` (Linux), `.dmg` (macOS, Apple Silicon
  only), `.msi` / `-setup.exe` (Windows), plus a `SHA256SUMS` covering all of them
- **macOS is Apple Silicon only, on purpose.** No maintainer has an Intel Mac, so
  an architecture-specific bug on Intel could not be reproduced or fixed. Shipping
  a binary nobody can debug is worse than not shipping it, so the build is native
  to the `macos-latest` runner (arm64) rather than universal. Intel users build
  from source. Revisit if someone on the team gets Intel hardware: give the
  macOS row `--target universal-apple-darwin`, add `rustup target add
  aarch64-apple-darwin x86_64-apple-darwin` for that row, and point `BUNDLE_DIR`
  at `src-tauri/target/universal-apple-darwin/release/bundle` - passing
  `--target` relocates Tauri's bundle output under the triple. That last part is
  the easy one to miss.
- `fail-fast` is off so every platform reports its own failure, but `release`
  depends on all three builds, so a partial artifact set is never published.
- **The macOS app is ad-hoc signed; nothing is notarized, and Windows is
  unsigned.** Tauri signs the app itself, from `bundle.macOS.signingIdentity` in
  `tauri.conf.json`, which happens before the DMG is built. Do not move signing
  after the build: v0.5.0-rc1 shipped with no identity set, so Tauri skipped
  codesign and the bundle carried only the Rust linker's ad-hoc signature on the
  inner binary. That combination is *invalid* rather than merely unsigned, and
  macOS reports it as "damaged and can't be opened" with no way for the user to
  click past it, unlike the ordinary unnotarized warning
  ([#350](https://github.com/Stalagmight/Stalagmight/issues/350)). The `build`
  job re-checks the app *inside* the DMG via `scripts/verify-macos-signature.sh`,
  since that is the artifact people download, and `scripts/mac-build.sh` runs the
  same script for the same reason. Real Developer ID signing plus notarization
  needs a paid Apple Developer account and is an open decision on
  [#226](https://github.com/Stalagmight/Stalagmight/issues/226).
- **What the signature does and does not buy.** `codesign --verify --deep --strict`
  proves the bundle is internally consistent, which is what makes it launchable at
  all. It proves nothing about *who* built it: an ad-hoc signature has no
  certificate and no Team ID, and anyone able to influence the bundle can re-sign
  it ad-hoc with no key. Integrity against build breakage, not authenticity. Read
  "signature verified" in a release log accordingly.
- **Signing also turns on the hardened runtime, with no entitlements.** Tauri's
  `hardenedRuntime` defaults to `true`, so giving it a `signingIdentity` opts the
  shipped app into a restricted runtime it never had before (the signature reports
  `flags=0x10002(adhoc,runtime)`). That is deliberate here, not incidental: it is
  verified working (the app launches and WKWebView spawns its content processes),
  it keeps the artifact forward-compatible with notarization, and the binary links
  only Apple frameworks with no bundled dylibs, so library validation has nothing
  to reject. If a future dependency needs JIT, a loadable plugin, or a spawned
  helper, that assumption has to be revisited - either add entitlements or set
  `"hardenedRuntime": false` explicitly rather than discovering it in a crash.
- The release notes get an "Installing" footer telling recipients how to get past
  Gatekeeper and SmartScreen. Because those steps talk people through overriding
  a security warning, the release also ships a `SHA256SUMS` generated on the
  runner that built the binaries, and the footer points at it.
- `verify` runs the test suite on Linux only. Per-platform test runs are tracked
  in [#325](https://github.com/Stalagmight/Stalagmight/issues/325); note Windows
  needs `STALAGMIGHT_TEST_MANIFEST=1` (see `scripts/test.ps1`).

**Runner cost**: macOS and Windows runners bill at a multiple of the Linux rate,
so a tag is materially more expensive than it used to be. Every job carries a
`timeout-minutes` for the same reason - the platform default is six hours, which
on the macOS runner is a large bill for a hung build.

Measured on a cold rehearsal run, 2026-07-29 (all three legs run in parallel, so
wall-clock for the whole release is roughly `verify` + the slowest leg):

| Job | Wall clock | Billing multiplier | Included minutes |
|-----|-----------|--------------------|------------------|
| `verify` (Linux, cached) | 4m03s | 1x | 4 |
| `build` linux | 17m11s | 1x | 17 |
| `build` macos | 10m16s | 10x | 103 |
| `build` windows | 27m47s | 2x | 56 |
| `release` | ~1m | 1x | 1 |
| **Total** | **~33m elapsed** | | **~181** |

macOS dominates the bill despite being the fastest leg. Re-measure rather than
trusting this table if the build changes shape; confirm the multipliers and
per-minute rates against GitHub's billing docs, which move.

### Rehearsing before you tag

The workflow also has a `workflow_dispatch` trigger. A manual run executes
`verify` and the whole `build` matrix but skips `release` (it is gated on
`startsWith(github.ref, 'refs/tags/')`), so the per-platform bundle paths and
globs can be proven without publishing anything:

```bash
gh workflow run release.yml --ref <branch>
gh run watch
```

Do this before the first tag after any change to the build matrix. The
platform-specific `bundle_dir` values and artifact globs are only really
confirmed by a run that produces the files.

A rehearsal covers everything except the `release` job, so two things stay
unproven until a real tag: `actions/download-artifact` and the publish step
itself. A rehearsal also bakes no candidate identity: only a tag ref is
classified, so a dispatch from a branch builds like a final.

The cheap real tag is a candidate. It runs the whole `release` job, publishes
`latest-beta.json` and the weekly mirror, and reaches Beta installs directly,
while `latest-stable.json` is untouched until the final. So after any change to
the pipeline, cut a candidate first and watch its `release` job rather than
assuming.

### The one macOS check no workflow can do

`codesign --verify` proves the signature is well-formed. It does **not** predict
what Gatekeeper shows a user, because that is a separate assessment of a
*quarantined* download, and an unnotarized app is rejected either way - `spctl
--assess` returns `rejected` for our DMG by design. So the difference between the
dismissible "Apple could not verify" prompt and the terminal "damaged and can't be
opened" cannot be read off any CI log.

Before announcing a release, open the built DMG once on a Mac that did not build
it, and note which dialog appears:

```bash
# on a second Mac, after downloading the DMG from the release
open Stalagmight_<version>_aarch64.dmg
# drag the app out, then launch it and record the exact wording
```

Building the app locally makes this test lie: the launching Mac has already run
that exact code, and Gatekeeper caches its verdict per cdhash, so the app opens
where a stranger's Mac might not. If the dialog offers a way through (System
Settings ▸ Privacy & Security ▸ **Open Anyway**), the install instructions are
accurate. If it says "damaged", signing is not sufficient and the release needs
notarization ([#226](https://github.com/Stalagmight/Stalagmight/issues/226))
before it can be handed to anyone.

## Update artifacts and signing

The real `plugins.updater.pubkey` is committed in `tauri.conf.json` (#586),
and every release since v0.5.0 ships signed update artifacts.
`bundle.createUpdaterArtifacts` is still **not** set in the checked-in
config - it is enabled CI-side only, in the release workflow's `build` job.
`tauri-cli` signs every updater bundle whenever `createUpdaterArtifacts` is
on and fails the build without the private key, so turning it on in the
checked-in config would break a contributor's local `pnpm tauri build`.
Keeping enablement CI-side means local builds never need the signing key.

Each `build` leg derives whether updater artifacts are enabled for that run
(`UPDATER_ON`) from `plugins.updater.pubkey` in the checked-out
`tauri.conf.json` - non-empty means on, which is every run on this repo. The
off path (no pubkey: build exactly as before #579, with a `::warning::` in the
log) is kept so a fork that removes the key still builds. Each leg emits,
alongside the installers already listed above, a signed update bundle per
platform plus a detached `.sig` (Tauri signs every package type it bundles,
not only the updater's own artifact):

| Platform | Update artifact | Signature |
|----------|------------------|-----------|
| Linux | `.AppImage` | `.AppImage.sig`, plus `.deb.sig` and `.rpm.sig` on the installers |
| Windows | NSIS `-setup.exe` (the update path) and `.msi` | `-setup.exe.sig` and `.msi.sig` |
| macOS | `.app.tar.gz` | `.app.tar.gz.sig` |

Windows ships two installers, but only the NSIS one is the updater's install
path - Tauri's updater does not drive an MSI upgrade. The `.msi.sig` (and the
Linux `.deb.sig`/`.rpm.sig`) exist because Tauri signs whatever it bundles,
not because anything consumes them yet. When `UPDATER_ON` is true, the
`build` job's collect step asserts at least one `.sig` landed and the
`release` job's completeness check requires the full platform-specific set
above; both are skipped on the no-pubkey path, since no `.sig` exists to
require.

**The updater plugin is registered only when a pubkey is committed.**
`lib.rs` adds `tauri_plugin_updater` only when
`commands::app_update::updater_configured()` finds a non-empty
`plugins.updater.pubkey` in `tauri.conf.json`, because the plugin's own
`Config` has a required `pubkey` with no `Default`: a fork or branch without
the key would otherwise fail `Builder::build()` and not start at all. Without
the key, `check_for_app_update` answers "This build has no update signing
key, so it cannot check for updates", and nothing downloads or installs
without a successful check first. `scripts/check-updater-pubkey.mjs`
(`pnpm lint:updater-pubkey`, gated in CI) fails on a placeholder, malformed
base64, or a non-https/non-`github.com` endpoint.

What the app does with an update - when it checks, the background download,
installing on quit, and the platform exceptions - is in
[SPEC 2.10](SPEC.md#210-updates--telemetry). Two release-side facts follow
from it:

- **Publishing reaches installs without a click.** Once a track's manifest
  points at a build, every install on that track with "Update automatically"
  on downloads it at its next check (within about 4 hours, or at the next
  launch), and wherever updates install on quit (macOS and Linux, except a
  Mac whose app folder the user cannot write to) installs it at the next quit.
  A final reaches Stable and Beta installs, a candidate Beta ones. Treat a
  publish as shipping: a candidate reaches the smaller Beta audience first
  ([Rehearsing an update from a candidate](#rehearsing-an-update-from-a-candidate)).
  Repointing recalls nothing already downloaded: a running app holding a
  downloaded update skips its 4-hour check and installs those bytes at the next
  quit, and an installed build stays installed
  ([Rollback / repoint runbook](#rollback--repoint-runbook)).
- **Only some packages update themselves.** The updater replaces an
  AppImage, an NSIS-installed Windows app, or a macOS `.app` (the macOS
  entry is in a manifest only for a Developer ID-signed release, and only for
  Apple silicon, `darwin-aarch64`). A copy installed from a `.deb`, `.rpm` or
  `.msi`, and a release binary with no bundle marker (a bare `cargo build`),
  refuse in `self_update_refusal` (`app_update.rs`) before the manifest is
  read. The refusal text is not shown to the user: About says "Could not
  check for updates" and automatic checks stay silent, so such users must be
  told to update through their package some other way.

### Keypair custody

The signing keypair is a minisign key generated and held by the maintainer
outside this repository. **The private key is never committed, never logged, and lives
only in the `TAURI_SIGNING_PRIVATE_KEY` GitHub secret** (plus its optional
password in `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). Losing it is not
recoverable in place: every already-installed app trusts the matching public
key baked into its own binary, so a lost or rotated private key strands
every existing install - they can no longer verify (and therefore will not
apply) any future signed update, and must be reinstalled from a fresh
download carrying the new public key.

### Secrets

| Secret | Required for | Effect when unset |
|--------|---------------|---------------------|
| `TAURI_SIGNING_PRIVATE_KEY` | Every `build` leg (the pubkey is committed) | The `build` job fails fast with a named error. Tauri itself already refuses to sign without this key - this guard exists only to fail earlier with a clearer, named message instead of chasing Tauri's own error. Only the no-pubkey path (a fork without the key) skips it |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Only if the key above has a password | Passed through empty otherwise |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Real Developer ID signing + notarization of the macOS leg | Absent (or `APPLE_SIGNING_IDENTITY` empty) leaves the macOS leg exactly as it was before #579: ad-hoc signed via the checked-in `signingIdentity: "-"`, not notarized |

**These are `release` environment secrets, not repository secrets.** Any
workflow pushed on any branch can read a repository secret, and these sign
builds under the maintainer's identity. The `build` job declares
`environment: release`, and the environment has a required reviewer with
admin bypass off, so every run that reaches `build` (a tag push or a
dispatch rehearsal) waits for that reviewer's approval in the Actions UI
before any signing secret is exposed. Set them with
`gh secret set <NAME> --env release`, never at repo level.
`TELEMETRY_ENDPOINT` is not signing material and stays a repository secret.

If the `APPLE_*` secrets are absent the macOS leg's graceful-degradation conditional keeps today's
ad-hoc-signing behavior byte-identical: the `build` job only appends a
`signingIdentity` override to the Tauri build config when
`APPLE_SIGNING_IDENTITY` is non-empty, and `scripts/verify-macos-signature.sh`
only runs the additional `spctl -a -vv` / `xcrun stapler validate` checks
under that same condition. If `APPLE_SIGNING_IDENTITY` is set but any of
`APPLE_ID`, `APPLE_PASSWORD` or `APPLE_TEAM_ID` is missing, the `build` job
fails fast and names the missing secret(s) - a partial set would otherwise
produce a signed-but-unnotarized bundle that fails much later, obscurely,
inside `xcrun stapler validate`. A job-internal `MACOS-SIGNED` marker
(`developer-id` or `adhoc`) records which path a given `build` run took; the
`release` job reads it, captures the value into a step output, and deletes
the file before publishing, so it never appears as a downloadable release
asset. The consumer of that output is the "Publish updater manifest" step -
see [Updater manifests](#updater-manifests) - which treats either signing
mode as "a macOS build exists" and an absent marker as "no macOS build to
publish".

## Updater manifests

Two update tracks exist, each user-selectable in Settings
([#650](https://github.com/Stalagmight/Stalagmight/issues/650)):

| Track | Gets | Manifest |
|-------|------|----------|
| **Stable** (the default) | final releases only | `latest-stable.json` |
| **Beta** | release candidates, then the final they lead to | `latest-beta.json` |

A build from a candidate tag starts on Beta (it is built with
`STALAGMIGHT_CHANNEL=beta`, see [Release candidates](#release-candidates));
every other build starts on Stable. Switching back to Stable does not
downgrade: an install on a candidate stays on it until the next final, which
is newer.

A third file, `latest-weekly.json`, is a byte-for-byte copy of
`latest-beta.json`, rewritten on every Beta publish. It exists only for 0.5.0
and 0.5.1 installs set to the retired Weekly track, which read it - see [The
retired weekly track](#the-retired-weekly-track).

Which tag publishes what:

| Tag | Manifest version | Publishes | The build bakes |
|-----|------------------|-----------|-----------------|
| `vX.Y.Z` (final) | `X.Y.Z` | `latest-stable.json`, `latest-beta.json`, `latest-weekly.json`, each track only where it is missing or older than the final | nothing |
| `vX.Y.Z-rc.N` (candidate) | `X.Y.Z-rc.N` | `latest-beta.json`, `latest-weekly.json` | `STALAGMIGHT_PRERELEASE=rc.N`, `STALAGMIGHT_CHANNEL=beta` |
| anything else (`-rcN`, `-weekly.*`, any other suffix) | (none) | nothing, with a `::notice::` | nothing |

The artifacts are plain `X.Y.Z` in every row. `scripts/release-channel.sh
classify <tag>` is the one definition of this table: the `verify` job, the
`release` job's preflight and publish steps, `release-tag.sh` and the repoint
workflow all ask it, so none of them can disagree about a tag shape, and
`test-release.sh` pins its output.

Each manifest is a static JSON file published as an asset on **`updater`**: a
rolling GitHub prerelease with no release notes of its own, created **once, by
hand**, per issue #579 (`gh release create updater --prerelease --title
"Updater manifests" --notes "Rolling manifests for the auto-updater. Not a
release - see docs/releasing.md."`). Nothing in this repo's automation ever
creates that release; every workflow that touches it only uploads to it with
`--clobber`, and fails loudly rather than creating it if it is missing. This
is deliberate: an accidentally-auto-created `updater` release with the wrong
visibility or the wrong tag is a worse failure mode than a loud error someone
has to act on once.

### How a manifest gets published

Before anything is made public, a **"Preflight rolling updater release" step**
runs early in the `release` job (right after "Determine updater enablement",
well before `gh release create`): when `UPDATER_ON` is true and the tag
publishes a manifest at all (a final or a candidate), it confirms `gh release
view updater` succeeds and fails loudly - naming this doc - if it does not. This
has to happen *before* the tag's own release goes public: the "Publish
updater manifest" step's own identical check (kept as a backup for the race
where the release disappears in between) runs far too late to prevent the bad
outcome. By the time it fires, `gh release create` has already made the
versioned release public, so a missing/renamed `updater` release strands a
now-public release with no manifest update, and a re-run cannot recover by
re-running the whole job - `gh release create` refuses a tag that already
exists.

For a candidate the preflight also keeps **Beta forward-only**: when
`latest-beta.json` already exists on `updater`, its `version` must be lower
than the candidate's (`scripts/release-channel.sh is-newer`), or the step fails
before anything is public. That catches an `rc.1` pushed after `rc.2`, and a
candidate for a version whose final already shipped. A missing
`latest-beta.json` (the first candidate ever) passes; one whose version cannot
be read fails closed. The error names the [repoint
workflow](#rollback--repoint-runbook), the deliberate way to move a track
backwards. A final is not checked here: it always publishes its release, and
the publish step decides which tracks it moves; see [Candidate versions and
the forward-only Beta track](#candidate-versions-and-the-forward-only-beta-track).

The `release` job's "Publish updater manifest" step (`.github/workflows/release.yml`)
runs after `gh release create`, once the tag's own release and its artifacts
exist. It:

1. Skips, with an `::notice::`, when updater artifacts are CI-side disabled
   (no `plugins.updater.pubkey` yet - see [Update artifacts and
   signing](#update-artifacts-and-signing)).
2. Routes on the job's tag classification (the table above), from the same
   strict, anchored patterns the preflight and the repoint workflow use. A
   tag that publishes nothing (the old `-rcN` shape, a weekly-looking tag,
   any other suffix) logs `no updater manifest for tag` as a `::notice::`
   and exits cleanly. **A candidate is offered to Beta installs only; Stable
   never sees one.**
3. Reconciles the manifest version against what was actually **built**,
   before trusting the tag at all. The version files, not the tag, decide
   the `X.Y.Z` a build carries, so a hand-pushed `v0.5.2-rc.1` on a tree
   still at `0.5.1` builds `0.5.1` artifacts while the manifest would claim
   `0.5.2-rc.1` - updater clients would see that as newer than what they
   run, download it, and relaunch the exact same build forever. The step
   parses the version baked into the artifacts from their filenames
   (`Stalagmight_X.Y.Z_...` - the same pattern the completeness check
   globs), asserts every artifact agrees, and requires it to equal the
   tag's base `X.Y.Z`, for a final and a candidate alike (a candidate's
   artifacts are plain `X.Y.Z` too). Any mismatch is a named `::error::` and
   the step exits 1 rather than publish a manifest that would loop clients
   on a stale build.
4. Reads the `MACOS-SIGNED` marker's captured value (see [Update artifacts
   and signing](#update-artifacts-and-signing)) to decide `--include-darwin`
   - **only `developer-id` enters the manifest.** `adhoc` is excluded with a
     `::warning::`, never treated as a lesser-but-acceptable variant: an
     ad-hoc build carries no verifiable identity, and the updater channel is
     trusted, so shipping an ad-hoc `.app.tar.gz` through it would let the
     updater silently replace a notarized install with an ad-hoc one on
     every existing user's machine. When `UPDATER_ON` is true and the marker
     never arrived at all (empty, not `developer-id` or `adhoc`), the step
     fails loudly rather than guessing - a silent default there would
     publish a manifest with no macOS entry, or the wrong one, on an
     otherwise-green run.
5. Runs `scripts/gen-updater-manifest.mjs` once against the job's own
   `release-artifacts/` directory, with the manifest version from the table
   above, reusing the same rendered `release-body.md` that `gh release
   create` already published as this release's notes. It then copies the
   result to every other file the tag publishes, so they are byte-identical,
   `pub_date` included. For a final it first reads the current
   `latest-stable.json` and `latest-beta.json` and writes each track only when
   its manifest is missing or older than the final. A skipped Stable gets a
   `::warning::` naming both versions and the repoint workflow; a skipped Beta
   (with its weekly mirror) a `::notice::`.
6. Uploads them all to the `updater` release in one `gh release upload
   --clobber` - each publish fully replaces the previous content of every
   file it writes.

### `scripts/gen-updater-manifest.mjs`

Pure Node, no dependencies. Given `--dir`, it finds each platform's update
artifact and its detached `.sig` (the Tauri v2 updater's own artifact set,
**not** every installer `build` produces):

| Platform key | Artifact | Signature |
|--------------|----------|-----------|
| `linux-x86_64` | `*.AppImage` | `*.AppImage.sig` |
| `windows-x86_64` | `*-setup.exe` (the NSIS installer - **not** the `.msi`, which Tauri's updater cannot drive) | `*-setup.exe.sig` |
| `darwin-aarch64` | `*.app.tar.gz` | `*.app.tar.gz.sig` (only when `--include-darwin true`) |

It inlines each `.sig` file's content verbatim as that platform's
`signature`, builds the asset URL as
`https://github.com/Stalagmight/Stalagmight/releases/download/<tag>/<file>`
(filename URL-encoded; the `Stalagmight/Stalagmight` base is hardcoded, since
this is a single-repo tool that only ever runs from this repo's own CI), and
hard-fails - naming what is wrong - on:

- a missing artifact or signature
- an ambiguous match (more than one file fits a platform's pattern)
- a `--version` other than `X.Y.Z` or `X.Y.Z-rc.N`, or a `--tag` other than
  `vX.Y.Z` or `vX.Y.Z-rc.N` (N from 1, no leading zero - the same two shapes
  `release-channel.sh` classifies as publishing; `--tag` flows into a URL path
  segment, and the script is also exposed as a bare `pnpm` command a caller
  could invoke by hand)
- an empty signature file, **or one that is not shaped like a real minisign
  signature** (an `untrusted comment:` header line followed by a base64
  body line - Tauri's signer's own format). A merely-non-empty check let a
  truncated or corrupted `.sig` through, which ships a manifest that fails
  closed on every client trying to verify it.

The manifest is deterministic and pretty-printed with a fixed key order
(`version`, `notes`, `pub_date`, then `platforms` in the table's order), so a
diff between two manifests - during review, or while debugging a bad
publish - is actually readable. Run its fixture tests with
`node scripts/gen-updater-manifest.test.mjs` (also `pnpm
gen:updater-manifest:test`, gated in both the leak-scan workflow, alongside
the pubkey tripwire, and the release workflow's `verify` job - a tag push
must not reach the paid build matrix with a broken generator either).

### Rollback / repoint runbook

`.github/workflows/repoint-updater-manifest.yml` (`workflow_dispatch` only,
`contents: write`, guarded to this repository) exists for the day a manifest
needs to point somewhere other than what the automatic publish wrote - most
often "stable regressed, repoint `latest-stable.json` back at the previous
release." Given a `track` (`stable` or `beta`), a past `tag`, and an explicit
`include-darwin` input, it downloads that release's update artifacts + sigs,
regenerates the manifest with that release's own notes (`gh release view <tag>
--json body`), and clobber-uploads it - the same file the automatic publish
step would have written for that tag, republished now.

The tag has to be one the track could have published, checked through the
same `release-channel.sh`: `stable` takes a bare `vX.Y.Z` only, `beta` a bare
tag or a `vX.Y.Z-rc.N` candidate, and the manifest version follows the table
above (`X.Y.Z-rc.N` for a candidate). Repointing `beta` rewrites the
`latest-weekly.json` mirror with it. Unlike the release job it applies no
forward-only check: moving a track backwards is what it is for.

**`include-darwin` is a required choice input with no default, never
inferred from whether a `.app.tar.gz` pair merely exists on the release.**
An ad-hoc macOS build can be present there too (see the developer-id-only
predicate above); "the asset exists" and "it is safe to publish through the
trusted updater channel" are different questions, and only the operator -
checking the original `build` run's log for the signing mode it recorded -
can answer the second one. Get this wrong and the dispatch itself becomes
the very failure the [developer-id-only predicate](#how-a-manifest-gets-published)
exists to prevent.

**Repoint first, delete after - never the other way around.** Updater
clients read the manifest, not the release; deleting a bad release before
repointing the manifest away from it leaves clients pointed at a dead
download URL, which is a worse failure than the regression the rollback was
meant to fix. Once the manifest points at a release you are willing to stand
behind, the bad release can be deleted (or left as a pre-release, unlinked)
at your discretion.

**Repointing does not recall clients already on the bad build.** Tauri's
updater only ever offers a strictly newer version than the one currently
installed; it never downgrades or re-prompts an install that already
applied the bad update. Repointing the manifest only stops **not-yet-updated**
clients from receiving it - anyone who updated before the repoint stays on
the bad build until the next good release reaches them normally.

**This dispatch is also the only retry path for a release job that failed
partway through.** `gh release create` and `gh release upload updater
--clobber` are two separate steps; a failure between them (or a failure
inside the manifest-generation step itself) leaves the tag's own GitHub
release published with the previous manifests left in place - they never
rotated to the new tag. There is no re-run of just the failed step:
re-running the `release` job would try to `gh release create` a tag that
already exists and fail immediately. Repoint each track the tag should have
published to (a final: `stable` and `beta`; a candidate: `beta`) at the tag
that already has its artifacts published instead.

```bash
gh workflow run repoint-updater-manifest.yml \
  -f track=stable -f tag=v0.5.1 -f include-darwin=true
gh workflow run repoint-updater-manifest.yml \
  -f track=beta -f tag=v0.5.2-rc.1 -f include-darwin=true
```

### Candidate versions and the forward-only Beta track

A candidate's manifest version is `X.Y.Z-rc.N`, a semver pre-release of the
version it leads to, so every step of a release train is strictly newer than
the one before it: `0.5.1` < `0.5.2-rc.1` < `0.5.2-rc.2` < `0.5.2`. That is
the ordering the updater's "only ever offer something newer" comparison needs,
and it is why the rc number is dotted (`rc.10` > `rc.9`, but `rc10` < `rc9`).
It is also why the build bakes `STALAGMIGHT_PRERELEASE`: an `rc.1` install that
believed it was plain `0.5.2` would never be offered `0.5.2-rc.2` (lower) or
the final (equal).

Both tracks only move forward on their own:

- **A candidate** that is not newer than what `latest-beta.json` already
  announces is refused at preflight, before anything is public. A candidate
  has no other purpose than reaching Beta, so there is nothing to publish
  without it.
- **A final** is never refused. Its GitHub release always publishes, and the
  publish step then writes each track only where that moves it forward:
  `latest-stable.json` when it is missing or older than the final, and
  `latest-beta.json` plus the weekly mirror on the same rule. A `v0.5.3`
  maintenance release cut after `v0.6.0` publishes as a release and leaves
  Stable on `0.6.0`, with a `::warning::` naming both versions and the repoint
  workflow (Stable installs are not offered it). A `0.5.2` hotfix cut while
  `0.5.3-rc.1` is out reaches Stable and leaves Beta on the candidate, with a
  `::notice::`. These checks run in the publish step, just before the upload.
  A manifest whose version cannot be read leaves its track alone, the other
  track is still published, and the step then fails, naming the file to check.

To move either track backwards on purpose, use the [repoint
workflow](#rollback--repoint-runbook).

Not covered: **two release runs publishing at once.** Each check reads
`latest-beta.json` before its own upload, so whichever run uploads last wins.

### The retired weekly track

0.5.0 and 0.5.1 offered a Weekly track in Settings that reads
`latest-weekly.json`, but the scheduled weekly builds (#579 part C) never
landed and no weekly tag exists, so an install set to Weekly was offered
nothing at all. Beta replaces it:

- Every Beta publish, and every repoint of `beta`, also writes
  `latest-weekly.json` as a byte-identical copy, so a 0.5.0 or 0.5.1 install
  still set to Weekly is offered candidates and finals exactly like a Beta one.
  Those versions compare with the updater plugin's default plain-semver
  ordering, which already ranks `0.5.1` < `0.5.2-rc.1` < `0.5.2`.
- A config that says `update_track = "weekly"` is read as Beta from 0.5.2 on
  (see [configuration.md](configuration.md#update-track)).
- `release-tag.sh` refuses weekly-shaped tags, and a hand-pushed one publishes
  nothing. Its weekly-supersession note (a final whose base is below some
  `v*-weekly.*` tag's) is still there, but it cannot fire while no weekly tag
  exists.
- Scheduled weekly or nightly builds are out of scope: each release build costs
  about 181 billed minutes and a manual approval, and a candidate cut on demand
  serves the same purpose.

### Rehearsing an update from a candidate

A candidate reaches Beta installs directly, so the update path is rehearsed
against real signed artifacts just by cutting one - no hand-made manifest:

1. Tag and push `vX.Y.Z-rc.N` as usual. The release job publishes
   `latest-beta.json` and the weekly mirror pointing at it.
2. On an install of an earlier version, switch Settings to Beta (a 0.5.0 or
   0.5.1 install can stay on Weekly) and check for updates: it is offered the
   candidate.
3. A Stable install is not offered it. `latest-stable.json` changes only on a
   final, so it is the check that the candidate stayed off Stable.

[#650](https://github.com/Stalagmight/Stalagmight/issues/650) has the full
0.5.2 test plan: rc.1 from a 0.5.1 Weekly install, rc.1 to rc.2 on Beta, rc.2
to the final, and Stable untouched throughout.

### Local end-to-end updater test

Exercises check -> download -> verify -> install against a throwaway key and a
manifest served from `localhost`, without the real signing key or GitHub
Releases. Everything here is local and disposable; none of it is committed.

```bash
# 1. A throwaway minisign keypair (never the real one). An empty password is
#    fine. Prints the public key and also writes it to <key>.pub.
pnpm tauri signer generate -w /tmp/stalagmight-updater-test.key

# 2. Export the private key BEFORE building: with createUpdaterArtifacts on,
#    `tauri build` signs each updater bundle itself and fails without the key.
export TAURI_SIGNING_PRIVATE_KEY=/tmp/stalagmight-updater-test.key
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=

# 3. Temporarily put the step 1 public key in plugins.updater.pubkey in
#    src-tauri/tauri.conf.json (do not commit it), then build with updater
#    artifacts on, using the same --config merge the release workflow uses.
#    The bundle and its .sig land under src-tauri/target/debug/bundle/.
pnpm tauri build --config '{"bundle":{"createUpdaterArtifacts":true}}' --debug

# 4. Write latest.json next to the bundle. The platforms key is Tauri's
#    {os}-{arch} form (linux-x86_64, windows-x86_64, darwin-x86_64,
#    darwin-aarch64), not a Rust target triple: a triple matches nothing, the
#    check reports "up to date", and the recipe silently tests nothing.
#    { "version": "9.9.9", "notes": "local test", "pub_date": "<RFC 3339>",
#      "platforms": { "linux-x86_64": { "signature": "<.sig file contents>",
#        "url": "http://127.0.0.1:8000/<bundle file name>" } } }

# 5. Serve that directory (it keeps running; use a second terminal for step 6).
python3 -m http.server 8000

# 6. Run the packaged build from step 3, launched from its bundle output,
#    with the variable set, e.g. on Linux:
STALAGMIGHT_UPDATE_URL=http://127.0.0.1:8000/latest.json \
  src-tauri/target/debug/bundle/appimage/<AppImage file name>
```

The launch check runs after about 10 seconds, or press About's "Check for
updates". The packaged build exercises the whole cycle: check, download, signature check,
and the install (at quit on macOS and Linux, or through "Restart to update").

`STALAGMIGHT_UPDATE_URL` (`UPDATE_URL_ENV` in `app_update.rs`) is honoured
verbatim only in a debug build, which is what lets a plain
`http://127.0.0.1` URL work here. A release build accepts it only as an
https `github.com` URL and otherwise ignores it (`is_trusted_manifest_url`).
Without the variable, a debug build refuses to check, download or install at
all (#644), so `./scripts/dev.sh` never updates itself from the real
manifest.

**Do not download an update into `./scripts/dev.sh`.** With the variable set,
a `tauri dev` build updates like any other: "Update automatically" is on by
default, so the launch check downloads at once, and on macOS and Linux a quit
or a restart then installs over `target/debug`. To check the manifest and the
banner from a dev build, turn "Update automatically" off in Settings first and
start the app with the variable only after that, then stop at the banner's
Download.

## Testing the release tooling

```bash
./scripts/test-release.sh
```

Covers all four scripts. For `release.sh` it clones the repo to a temp directory and
drives the real script through a normal bump, a `--no-tag` bump, and the refusal
or recovery cases (re-releasing the current version, an existing tag, a drifted
version-string format, an empty `[Unreleased]`, a `Cargo.lock` already out of step,
a `Cargo.lock` whose stanza is missing entirely, a failing `$EDITOR`, and a failing
or missing license-bundle generator), plus a CRLF checkout, asserting each time that
the tree is left clean and nothing was tagged. `RELEASE_GEN_THIRD_PARTY` points every
case at a stub generator that records the `Cargo.lock` it saw, which is how case 1
proves the bundle is regenerated after the bump. For `release-tag.sh` it drives the
whole bump-then-tag flow in a clone, then each refusal above (the `-rcN`, other-suffix
and weekly shapes included), asserting every time that the run failed, said why, and
left no tag. For `release-notes.sh` it runs the real script against a
fixture changelog and checks the section each tag shape resolves to, the
candidate banner, and that an empty changelog still yields a usable body. For
`release-channel.sh` it pins the full classification of finals, `rc.N` candidates
and every shape that publishes nothing, and the version ordering the Beta guard uses.

Run it after touching any of them, including `lib/release-version.sh`, which the
two entry points share; it takes a few seconds and needs no network.

## Version locations

The version string lives in 4 tracked files. `release.sh` updates all of them:

| File | Field | Bumped by `release.sh`? |
|------|-------|-------------------------|
| `package.json` | `"version"` | yes |
| `src-tauri/tauri.conf.json` | `"version"` | yes |
| `src-tauri/Cargo.toml` | `version` | yes |
| `src-tauri/Cargo.lock` | the `name = "stalagmight"` entry | yes |

`Cargo.lock` records the crate's own version, and because nothing here builds with
`cargo --locked`, a mismatch never fails a build - the next `cargo` invocation just
rewrites the lock in place, leaving an unexplained dirty file in a tree the release
reported as clean. That is how the lock sat at `0.2.0` while everything else said
`0.5.0`. `release.sh` bumps it now
([#352](https://github.com/Stalagmight/Stalagmight/issues/352)), rewriting only the
line after `name = "stalagmight"` - every dependency has a `version` line too, so a
blanket substitution would rewrite the whole file. The pre-flight refuses to start
if the lock is already out of step with `package.json`, rather than quietly bumping
past a discrepancy worth looking at.

The About dialog reads the version dynamically via `__APP_VERSION__` (injected by Vite: the matching release tag on HEAD when there is one, otherwise `package.json`'s version plus the commit hash).

## Troubleshooting

### "uncommitted changes" error
Stash or commit changes first. The script allows uncommitted `CHANGELOG.md` but nothing else.

### "[Unreleased] section is empty" error
Write release notes into the `## [Unreleased]` section of `CHANGELOG.md` before running with `--no-edit`.

### "Cargo.lock records stalagmight at X, but package.json says Y"
The lock drifted out of step before the release started, so the script stops rather
than bumping past a discrepancy worth looking at. Sync it and re-run: the simplest
fix is any `cargo` command in `src-tauri` (`cargo metadata >/dev/null` is enough),
which rewrites the version in place, then commit the lock. `--dry-run` reports this
too, so it is safe to check before you start.

### "Cargo.lock has N lines matching name = "stalagmight"" / "no version = line"
A shape problem rather than a value one - the rewrite anchors on the single
`name = "stalagmight"` stanza with its `version` on the following line. Look at
`src-tauri/Cargo.lock` before doing anything else; nothing was modified.

### "regenerating THIRD-PARTY-LICENSES.md (...) failed"
The version files and the bundle are reverted; nothing was committed. The generator
needs `cargo-about` 0.9.1 (`cargo install cargo-about --version 0.9.1 --locked
--features cli`) and a fresh `pnpm install --frozen-lockfile`; its own error above
the message says which is missing. Fix that and re-run.

### `$EDITOR` exited non-zero
The version files are reverted automatically, but `CHANGELOG.md` keeps the moved
`[Unreleased]` content so your notes survive. Review it and re-run, or
`git checkout -- CHANGELOG.md` to start over.

### Multi-platform builds
Linux, macOS and Windows are all built from the tag by the `build` matrix. Each
platform builds on its own runner because Tauri can't cross-compile - it needs
that platform's system libraries.

If one platform fails, the other two still build and report, but no release is
created. Fix the failing platform and re-run the workflow, or delete and re-push
the tag.

### `release.sh` fails on macOS
Shouldn't anymore, but this is what to look for: the script used `grep -oP` and
GNU-style `sed -i`, neither of which exists in the BSD userland macOS ships. It
now reads the version with POSIX `sed` and rewrites files through a helper that
avoids `sed -i` entirely. **The script has to run under BSD userland, so don't
reach for GNU-only flags here** - `scripts/test-release.sh` runs on whatever
machine invokes it and will catch a regression.
