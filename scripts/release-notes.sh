#!/usr/bin/env bash
set -euo pipefail

# release-notes.sh - build the GitHub release body for a tag, on stdout.
#
# Used by .github/workflows/release.yml so the published notes and a local
# preview come from the same code. Run it before tagging to see exactly what
# will be published:
#
#   ./scripts/release-notes.sh v0.5.0
#   ./scripts/release-notes.sh v0.5.0-rc1
#
# A release candidate takes the notes of the version it is a candidate for:
# v0.5.0-rc1 reads the `## [0.5.0]` changelog section, because an RC is that
# release, built early. If that section does not exist yet - the usual case
# before `release.sh` moves it - it falls back to `## [Unreleased]`.
#
# Once that section exists, [Unreleased] is no longer read at all, so anything
# written there after the first candidate would ship unannounced. This warns on
# stderr when that is about to happen; see the check further down.

SCRIPT_DIR="$(dirname "$(realpath "$0")")"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
CHANGELOG="${CHANGELOG:-$PROJECT_ROOT/CHANGELOG.md}"

TAG="${1:-}"
if [ -z "$TAG" ]; then
  echo "Usage: $0 <tag>   (e.g. v0.5.0, v0.5.0-rc1)" >&2
  exit 1
fi

VERSION="${TAG#v}"
# 0.5.0-rc1 -> 0.5.0 ; 0.5.0 -> 0.5.0
BASE_VERSION="${VERSION%%-*}"
PRERELEASE=false
[ "$VERSION" != "$BASE_VERSION" ] && PRERELEASE=true

# Pull one "## [x.y.z]" section, stopping at the next "## " heading. Tolerates a
# bracketed or bare header, with or without a trailing date.
extract_section() {
  awk -v ver="$1" '
    $0 ~ "^## +\\[?" ver "\\]?([^0-9.]|$)" { found=1; next }
    found && /^## / { exit }
    found { print }
  ' "$CHANGELOG"
}

# Strip leading/trailing blank lines so the body does not open with whitespace.
trim_blank_lines() {
  awk 'NF {p=1} p' | awk '{lines[NR]=$0} END {last=NR; while (last>0 && lines[last]=="") last--; for(i=1;i<=last;i++) print lines[i]}'
}

BODY="$(extract_section "$BASE_VERSION" | trim_blank_lines)"
UNRELEASED="$(extract_section "Unreleased" | trim_blank_lines)"

if [ -n "$BODY" ]; then
  # The version's own section won, so anything still under [Unreleased] is in the
  # build being tagged but not in the notes describing it - and silently so. That
  # is exactly what happened between v0.5.0-rc1 and rc2: four fixes sat under
  # [Unreleased] while the tag read ## [0.5.0], and only reading the rendered body
  # before tagging caught it. Warn on stderr, never stdout: the workflow redirects
  # stdout into the release body, so anything printed there would be published.
  # Warn rather than fail - a late-breaking entry nobody wants to announce is a
  # legitimate choice, and failing here would block publishing at the last step.
  # Count only lines that carry content: a section holding nothing but `### Fixed`
  # and blanks has nothing to fold, and a warning that fires on a good state is
  # how a warning gets ignored. Headings are `#`-prefixed; NF drops blank lines.
  at_risk=$(printf '%s\n' "$UNRELEASED" | awk '!/^#/ && NF { n++ } END { print n+0 }')
  if [ "$at_risk" -gt 0 ]; then
    cat >&2 <<WARN
warning: ${at_risk} line(s) under '## [Unreleased]' will NOT be published.
         ${TAG} takes its notes from '## [${BASE_VERSION}]', so entries left under
         [Unreleased] ship inside the build with no mention in the release body.
         Fold them into '## [${BASE_VERSION}]' before tagging, or accept that they
         go unannounced.
WARN
  fi
else
  # Before release.sh moves it, the notes still live under [Unreleased]. Nothing to
  # warn about here - that content is what gets published.
  BODY="$UNRELEASED"
fi
if [ -z "$BODY" ]; then
  BODY="Release ${TAG}"
fi

if [ "$PRERELEASE" = true ]; then
  cat <<HEADER
> **Release candidate for ${BASE_VERSION}.** Not a final release - published so
> testers can try it. The notes below are the notes for ${BASE_VERSION}; anything
> in them may still change before that version ships.

HEADER
fi

printf '%s\n' "$BODY"

# Unnotarized builds interpose a warning on first launch. Tell recipients how
# to get past it, and what to check first. The ad-hoc macOS bullet leads with the xattr command on purpose: it works on every macOS
# version, whereas the Control-click override was removed in macOS 15, so the
# menu path differs by version and the command does not.
cat <<'FOOTER'

---

### Installing

The macOS build is **Apple Silicon only** (M1 and later). There is no Intel build:
no maintainer has an Intel Mac, so an architecture-specific bug there could not be
reproduced or fixed. Intel users should build from source instead.

FOOTER

# MACOS_SIGNING is the build job's MACOS-SIGNED marker, passed in by release.yml:
# `developer-id` when the macOS app was Developer ID signed and notarized,
# anything else (or unset, as in a local preview) for the ad-hoc fallback. The
# two must never mix: telling people to strip quarantine from a notarized app is
# needless security advice, and promising a clean launch for an ad-hoc one is false.
if [ "${MACOS_SIGNING:-adhoc}" = "developer-id" ]; then
  cat <<'MACOS'
The macOS app is signed with a Developer ID and notarized by Apple, so it opens
like any other downloaded app. The Windows and Linux builds are not signed, so
their first launch is blocked and you have to allow it once.

- **macOS**: open the DMG and drag Stalagmight to Applications. The first launch asks you to confirm opening an app downloaded from the internet; nothing needs to be overridden.
MACOS
else
  cat <<'MACOS'
None of these builds are notarized, so the first launch is blocked on every
platform and you have to allow it once. The macOS app is ad-hoc signed, which is
what lets it launch at all once allowed; Windows and Linux are unsigned.

- **macOS**: clear the quarantine flag, which works on every macOS version: `xattr -dr com.apple.quarantine /Applications/Stalagmight.app`. Alternatively, open it once from **System Settings > Privacy & Security**, where a blocked app appears with an **Open Anyway** button. On macOS 14 and earlier you can instead right-click the app and choose **Open**; that shortcut no longer works on macOS 15+.
MACOS
fi

cat <<'FOOTER'
- **Windows**: SmartScreen shows "Windows protected your PC". Choose **More info**, then **Run anyway**.
- **Linux**: install the `.deb` or `.rpm`, or `chmod +x` the `.AppImage` and run it.

Those steps ask you to override a security warning, so check what you downloaded
first. `SHA256SUMS` is attached to this release and was generated on the runner
that built the binaries:

```
sha256sum -c SHA256SUMS        # Linux
shasum -a 256 -c SHA256SUMS    # macOS
```
FOOTER
