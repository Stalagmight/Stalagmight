# Release Process

## Overview

Releases are created through Claude Code. The process:

1. Claude reviews git log since the last tag
2. Claude writes a human-readable summary into the `## [Unreleased]` section of `CHANGELOG.md`
3. Claude commits the changelog update
4. Claude runs `./scripts/release.sh <patch|minor|major> --no-edit --no-tag`
5. Claude tags with `./scripts/release-tag.sh vX.Y.Z[-rcN]`, then pushes the
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
./scripts/release-notes.sh v0.5.0
./scripts/release-notes.sh v0.5.0-rc1
```

The script picks the section, then appends the install instructions and the
checksum note that every release carries:

| Tag | Notes come from |
|-----|-----------------|
| `v0.5.0`, after `release.sh` moved the section | `## [0.5.0]` |
| `v0.5.0`, before that | `## [Unreleased]` (fallback) |
| `v0.5.0-rc1` | `## [0.5.0]`, or `## [Unreleased]`, plus a candidate banner |
| anything with no matching section | `Release <tag>` (last resort) |

A candidate deliberately reuses the notes of the version it is a candidate for -
an RC *is* that release, built early - so the notes are written once.

## Release candidates

Bump once, then cut as many candidates as you like before the real thing:

```bash
./scripts/release.sh 0.5.0 --no-edit --no-tag      # bump + changelog, no tag
git push
./scripts/release-tag.sh v0.5.0-rc1                # then rc2, rc3, ...
git push origin v0.5.0-rc1
./scripts/release-tag.sh v0.5.0                    # the real one
git push origin v0.5.0
```

`--no-tag` matters: without it `release.sh` creates `v0.5.0` immediately, which
would either publish the real release early or sit on the wrong commit once
further fixes land before `rc2`.

**Use `release-tag.sh` rather than a bare `git tag`.** The two scripts split the
work: `release.sh` owns a *new version* (bump the four files, move the changelog,
commit), `release-tag.sh` owns *tagging a tree already at that version* - which is
every candidate, and every release that follows one. A hand-typed `git tag` runs
none of the checks either script owns; that is how four fixes shipped unannounced
between `v0.5.0-rc1` and `rc2`
([#406](https://github.com/Stalagmight/Stalagmight/issues/406)). It refuses:

| Condition | Why |
|-----------|-----|
| the version files do not carry the tag's version | `v0.5.1-rc1` cut from a `0.5.0` tree builds `Stalagmight_0.5.0_*` artifacts and an About box reading `0.5.0` |
| the working tree is dirty | a tag names a commit, so uncommitted notes are not the notes that publish |
| the tag already exists | candidates are kept, not recycled - a tester's bug report names one |
| the tag is malformed | the accepted shape matches what `release.yml` will mark as a pre-release |
| the changelog has no notes for it | the published body would be the bare string `Release v0.5.0` |
| entries sit under `[Unreleased]` | they ship in the build and are named nowhere (`--allow-unannounced` to override) |

It also *notes*, without blocking, a final release that was never cut as a
candidate and a `## [X.Y.Z] - date` older than today - both legitimate, both
otherwise invisible.

It stops at a **local** tag and prints the push command. Pushing is the
irreversible step: it starts a paid three-platform build and, for a bare
`vX.Y.Z`, publishes as **Latest**. `--dry-run` does everything except create the
tag, which is the cheapest way to read the body you are about to publish.

- **The version files stay at the plain `0.5.0`.** The candidate lives only in
  the tag. This is not a style choice: the Windows MSI bundler rejects a
  non-numeric pre-release identifier, so `0.5.0-rc1` in `tauri.conf.json` fails
  the Windows build outright. Artifacts are therefore named
  `Stalagmight_0.5.0_*` for every candidate.
- **A tag with a suffix is published as a GitHub pre-release**, so it never
  becomes "Latest" and is not handed to people expecting the final build.
- **About shows the tag**, so a candidate reads `0.5.0-rc1` and cannot be
  mistaken for the final release. Keep candidates rather than deleting them -
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
./scripts/release-tag.sh vX.Y.Z-rcN --dry-run     # or vX.Y.Z for the real one

# 3. Tag the commit you mean, then push the tag.
./scripts/release-tag.sh vX.Y.Z-rcN
git push origin vX.Y.Z-rcN
```

A tag with a suffix publishes as a pre-release; a bare `vX.Y.Z` publishes as **Latest**.
Check the `## [X.Y.Z] - YYYY-MM-DD` date before the final tag: `release.sh` stamped it
the day the version was bumped, which may be well before the day it ships.
`release-tag.sh` says so when they differ, but only *you* can decide which date the
release should carry.

## What `scripts/release.sh` does

1. Reads current version from `package.json`
2. Computes new version (major/minor/patch or explicit X.Y.Z)
3. Verifies the working tree is clean via `git status --porcelain` - untracked files
   count, CHANGELOG.md is the one exemption, and a failing git stops the script
4. Verifies `## [Unreleased]` section is non-empty
5. Updates version in all 4 files that carry it (see [Version locations](#version-locations)):
   - `package.json`
   - `src-tauri/tauri.conf.json`
   - `src-tauri/Cargo.toml`
   - `src-tauri/Cargo.lock` (this crate's stanza only)
6. Moves `[Unreleased]` content under a new `## [X.Y.Z] - YYYY-MM-DD` header
7. Commits: `chore: release vX.Y.Z`
8. Tags: `vX.Y.Z`

### Flags

| Flag | Effect |
|------|--------|
| `--no-edit` | Skip opening `$EDITOR` on the changelog (required for Claude) |
| `--dry-run` | Print what would happen without making changes |
| `--no-tag` | Bump, move the changelog and commit, but create no tag - use when the next thing you cut is a candidate (`v0.5.0-rc1`) rather than the release |

## What `scripts/release-tag.sh` does

Tags a tree that is *already* at the right version - see [Release
candidates](#release-candidates) for what it refuses and why. It writes nothing
but the tag: no version files, no changelog, no push.

1. Checks the tag's shape, and that it does not already exist
2. Checks the working tree is clean
3. Checks all 4 version files carry the tag's base version (`v0.5.0-rc1` -> `0.5.0`)
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
| `verify` | `ubuntu-latest` | TypeScript check + Rust tests + a release-profile `cargo check --release`, plus the third-party license bundle gate (leak-scan runs on branch/PR events only, so a tag push would otherwise skip it) - mostly a subset of the CI `check` job, not the whole gate. It does not run `cargo fmt --check`, clippy, vitest, the file-size ratchets or the private-leak/em-dash scans; those gate the merge, and a tag is cut from already-merged code |
| `build` | `ubuntu-latest`, `macos-latest`, `windows-latest` | `pnpm tauri build`, collects that platform's bundles, checksums them, uploads them as a job artifact |
| `release` | `ubuntu-latest` | Downloads all three artifact sets, re-verifies the checksums, asserts every platform is represented, extracts the changelog section, creates the GitHub release via the `gh` CLI |

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
- **An ad-hoc signature changes identity every build, which the keychain notices.**
  The SpeleoDB token is stored via the OS keychain (`src-tauri/crates/speleodb/src/speleodb/auth.rs`),
  and a keychain item's ACL is bound to the creating app's code identity. An ad-hoc
  signature's designated requirement is its cdhash, which differs for every build,
  so each release is a different app as far as the keychain is concerned and users
  may be re-prompted for their token after updating. A real Developer ID gives a
  stable identity and would remove this; it is another argument for #226. A denial
  is handled safely rather than silently - the token degrades to session-only
  memory and is never written to disk unless plaintext storage was explicitly
  opted into.
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
itself. Watch the first tagged run's `release` job rather than assuming.

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

`bundle.createUpdaterArtifacts` is **not** set in `tauri.conf.json` - it is
enabled CI-side only, in the release workflow's `build` job, and only once a
real `plugins.updater.pubkey` is committed there (issue #579 provisioning).
This is deliberate, not incidental: `tauri-cli` base64-decodes that pubkey
with a hard failure at bundle time whenever `createUpdaterArtifacts` is on,
so turning it on in the checked-in config before the real key exists breaks
every build, on every platform, including a contributor's local
`pnpm tauri build`. Keeping enablement CI-side means local dev builds never
need a signing key at all, today or after the key lands.

Each `build` leg derives whether updater artifacts are enabled for that run
(`UPDATER_ON`) from `plugins.updater.pubkey` in the checked-out
`tauri.conf.json` - non-empty means on. While the pubkey is pending, every
build runs exactly as it did before #579 and a `::warning::` says so in the
log. Once enabled, each leg emits, alongside the installers already listed
above, a signed update bundle per platform plus a detached `.sig` (Tauri
signs every package type it bundles, not only the updater's own artifact):

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
above; both are skipped while the pubkey is pending, since no `.sig` exists
to require.

**The pubkey and the stable endpoint are committed** to `plugins.updater`,
and `lib.rs` registers the updater plugin whenever that key is there
(`updater_configured` in `src-tauri/src/commands/app_update.rs`).
`scripts/check-updater-pubkey.mjs` (`pnpm lint:updater-pubkey`, gated in CI)
checks the committed block on every run: a placeholder, malformed base64, or
a non-https/non-`github.com` endpoint fails the gate. A config without the
block passes it, and the app then starts with the updater off.

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
| `TAURI_SIGNING_PRIVATE_KEY` | Every `build` leg, once `plugins.updater.pubkey` is committed | The `build` job fails fast with a named error. Tauri itself already refuses to sign without this key - this guard exists only to fail earlier with a clearer, named message instead of chasing Tauri's own error. While the pubkey is pending this secret is not required at all |
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

Two update tracks exist, **stable** and **weekly**, each user-selectable at
runtime. Each track's manifest is a static JSON file, `latest-stable.json` or
`latest-weekly.json`, published as an asset on **`updater`**: a rolling
GitHub prerelease with no release notes of its own, created **once, by
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
matches either strict track shape (below), it confirms `gh release view
updater` succeeds and fails loudly - naming this doc - if it does not. This
has to happen *before* the tag's own release goes public: the "Publish
updater manifest" step's own identical check (kept as a backup for the race
where the release disappears in between) runs far too late to prevent the bad
outcome. By the time it fires, `gh release create` has already made the
versioned release public, so a missing/renamed `updater` release strands a
now-public release with no manifest update, and a re-run cannot recover by
re-running the whole job - `gh release create` refuses a tag that already
exists.

The `release` job's "Publish updater manifest" step (`.github/workflows/release.yml`)
runs after `gh release create`, once the tag's own release and its artifacts
exist. It:

1. Skips, with an `::notice::`, when updater artifacts are CI-side disabled
   (no `plugins.updater.pubkey` yet - see [Update artifacts and
   signing](#update-artifacts-and-signing)).
2. Derives the track from the tag using the same **strict, anchored** shapes
   the preflight step and the repoint workflow use - not a substring match:
   `^v[0-9]+\.[0-9]+\.[0-9]+-weekly\.[0-9]{8}$` -> `weekly`;
   `^v[0-9]+\.[0-9]+\.[0-9]+$` -> `stable`; anything else (an `-rc*`
   candidate, or a malformed weekly-looking tag like `v0.5.1-weekly.oops`)
   touches **neither** manifest - the step logs `no manifest for tag` and
   exits cleanly. A `*-weekly.*` substring match once let a malformed tag
   still clobber `latest-weekly.json`; the anchored regex is what closes
   that. **A release candidate is never offered to updater clients on
   either track.**
3. Reconciles the manifest version against what was actually **built**,
   before trusting the tag at all. Version-baking currently comes from
   whatever the tree already said at build time (proper tag-driven
   version-baking lands via `workflow_call` in a later PR), so a directly
   pushed weekly tag can build a stale tree version while the tag claims a
   newer one - a `0.5.0`-baked build published under a `0.5.1-weekly.<date>`
   manifest would have updater clients see `0.5.1-weekly.<date>` as newer
   than their installed `0.5.0`, download it, and relaunch the exact same
   `0.5.0` build forever. The step parses the version baked into the
   artifacts from their filenames (`Stalagmight_X.Y.Z_...` - the same
   pattern the completeness check globs), asserts every artifact agrees, and
   requires: for **weekly**, the tag's numeric base equals the artifact
   version; for **stable**, the tag version equals the artifact version
   exactly. Any mismatch is a named `::error::` and the step exits 1 rather
   than publish a manifest that would loop clients on a stale build.
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
5. Runs `scripts/gen-updater-manifest.mjs` against the job's own
   `release-artifacts/` directory, reusing the same rendered `release-body.md`
   that `gh release create` already published as this release's notes.
6. Uploads the result to the `updater` release as `latest-<track>.json`, with
   `--clobber` - each publish on a track fully replaces the previous manifest
   for that track.

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
- a malformed `--version` or a malformed `--tag` (both checked against the
  same `vX.Y.Z[-suffix]`/`X.Y.Z[-suffix]` shape `release-tag.sh` accepts -
  `--tag` flows into a URL path segment, and the script is also exposed as a
  bare `pnpm` command a caller could invoke by hand)
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
release." Given a `track`, a past `tag`, and an explicit `include-darwin`
input, it downloads that release's update artifacts + sigs, regenerates the
manifest with that release's own notes (`gh release view <tag> --json
body`), and clobber-uploads it - the same file the automatic publish step
would have written for that tag, republished now.

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
release published with no updated manifest, or with the previous track's
manifest left in place - `latest-<track>.json` never rotated to the new
tag. There is no re-run of just the failed step: re-running the `release`
job would try to `gh release create` a tag that already exists and fail
immediately. Repoint the track at the tag that already has its artifacts
published instead.

```bash
gh workflow run repoint-updater-manifest.yml \
  -f track=stable -f tag=v0.5.0 -f include-darwin=true
```

### The weekly version invariant

A weekly build's version carries a `-weekly.YYYYMMDD` suffix, which is a
semver **pre-release** identifier - `0.5.1-weekly.20260817` sorts *below*
plain `0.5.1`, not above it, so "just append `-weekly.<today>` to the last
stable's base" would be wrong: the first weekly cut after `0.5.1` ships would
compare as older than `0.5.1` itself and never reach anyone on that stable.
The scheme that later PR in the #579 arc implements instead bakes each
weekly onto the **next unconsumed base** - the base no stable release has
shipped yet. That keeps every weekly version strictly above the last stable
and strictly below the next one: after `0.5.1` ships, weeklies are cut as
`0.5.2-weekly.YYYYMMDD` until `0.5.2` itself ships, at which point they
converge - `0.5.2` is not "older" than `0.5.2-weekly.20260901`, it is the
release that pre-release series was building toward, and stable correctly
supersedes it.

### The weekly-track supersession warning

The weekly manifest, once that later PR starts cutting weekly tags on a
schedule, only ever moves forward: each publish clobbers the previous
`latest-weekly.json`. Given the invariant above, a problem only arises when
a weekly tag's base is **strictly greater** than the stable base being cut -
that means a weekly series already targets a version beyond the stable
about to ship, so weekly-track users on that build (or a later one in the
same series) will never be offered it; there is no mechanism for a stable
release to "supersede" a weekly build already aimed past it. At an *equal*
base the two converge as described above, and the stable correctly
supersedes the weekly series - not a condition to warn about.
`scripts/release-tag.sh` warns about exactly the strictly-greater case
(without blocking - it may be an intentional patch cut behind a weekly build
that shipped nothing users need), comparing versions numerically
component-by-component (so `0.5.10` correctly reads as greater than `0.5.9`,
not less, the way a lexical comparison would get wrong) and taking the
**highest** weekly base found across every `v*-weekly.*` tag, not merely the
first match. Weekly tags themselves are refused outright by
`release-tag.sh` - they are reserved for the automated weekly pipeline
(#579, lands separately), never hand-typed, and a hand-typed one could
collide with or masquerade as a real one.

## Testing the release tooling

```bash
./scripts/test-release.sh
```

Covers all three scripts. For `release.sh` it clones the repo to a temp directory and
drives the real script through a normal bump, a `--no-tag` bump, and seven refusal
or recovery cases (re-releasing the current version, an existing tag, a drifted
version-string format, an empty `[Unreleased]`, a `Cargo.lock` already out of step,
a `Cargo.lock` whose stanza is missing entirely, and a failing `$EDITOR`), plus a
CRLF checkout, asserting each time that the tree is left clean and nothing was
tagged. For `release-tag.sh` it drives the whole bump-then-tag flow in a clone,
then each refusal above, asserting every time that the run failed, said why, and
left no tag. For `release-notes.sh` it runs the real script against a
fixture changelog and checks the section each tag shape resolves to, the
candidate banner, and that an empty changelog still yields a usable body.

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

### "the working tree has changes other than CHANGELOG.md" error
Commit the listed files, or `git stash -u` (plain `git stash` leaves untracked `??`
files behind, and those block too). The script allows an uncommitted `CHANGELOG.md`
but nothing else. "git status failed" means git itself could not read the tree - fix
that first; the script will not guess that the tree is clean.

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
