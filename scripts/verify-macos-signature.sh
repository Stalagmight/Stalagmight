#!/usr/bin/env bash
#
# Verify that the .app inside a built DMG carries a valid code signature.
#
# Why this exists: v0.5.0-rc1 shipped a DMG whose .app macOS refused to open,
# reporting it as "damaged". The download was intact - its sha256 matched the
# published SHA256SUMS - the bundle had simply never been signed. With no signing
# identity configured, Tauri skips codesign entirely, leaving only the signature
# the Rust linker puts on the inner binary, which is inconsistent with the bundle
# wrapped around it: no sealed resources, no _CodeSignature directory. macOS
# treats that as an *invalid* signature rather than an unsigned one, and unlike
# the ordinary unnotarized-app warning, an invalid signature cannot be clicked
# past. See https://github.com/Stalagmight/Stalagmight/issues/350.
#
# Why it checks inside the DMG rather than the loose bundle next to it: the DMG is
# what people actually download, and the rc1 failure was precisely a divergence
# between the two. Signing used to run *after* the DMG had already been built from
# an unsigned copy, so the loose .app looked fine while the shipped one did not.
# Verifying the loose bundle would therefore pass on exactly the bug this guards.
#
# This proves the bundle is internally consistent - not that it is trustworthy.
# An ad-hoc signature carries no certificate and no Team ID, so it says nothing
# about who built the artifact, and the app is not notarized.
#
# When APPLE_SIGNING_IDENTITY is set (real Developer ID signing requested -
# see the release workflow's Build step), this also runs the checks an
# ad-hoc signature cannot pass: `spctl -a -vv` (Gatekeeper's own verdict) and
# `xcrun stapler validate` (confirms the notarization ticket is stapled to
# the bundle, so Gatekeeper can check it offline). Any failure here means
# real signing was requested but the shipped bundle would not pass Gatekeeper
# - that must fail the workflow, not degrade quietly to the ad-hoc checks.
#
# Usage:
#   verify-macos-signature.sh <path-to-dmg>
#   verify-macos-signature.sh <directory-containing-exactly-one-dmg>
set -euo pipefail

TARGET="${1:-}"
if [ -z "$TARGET" ]; then
  echo "usage: $(basename "$0") <path-to-dmg|directory-with-one-dmg>" >&2
  exit 2
fi

shopt -s nullglob

if [ -d "$TARGET" ]; then
  dmgs=( "$TARGET"/*.dmg )
  if [ "${#dmgs[@]}" -ne 1 ]; then
    echo "error: expected exactly 1 DMG in $TARGET, found ${#dmgs[@]}" >&2
    exit 1
  fi
  DMG="${dmgs[0]}"
else
  DMG="$TARGET"
fi

if [ ! -f "$DMG" ]; then
  echo "error: no such DMG: $DMG" >&2
  exit 1
fi

MNT="$(mktemp -d)"
cleanup() {
  hdiutil detach "$MNT" -quiet 2>/dev/null || true
  rmdir "$MNT" 2>/dev/null || true
}
trap cleanup EXIT

hdiutil attach "$DMG" -nobrowse -readonly -mountpoint "$MNT" >/dev/null

# Globbed rather than hard-coding Stalagmight.app, so renaming productName in
# tauri.conf.json does not silently turn this into a check that always fails.
apps=( "$MNT"/*.app )
if [ "${#apps[@]}" -ne 1 ]; then
  echo "error: expected exactly 1 .app inside $(basename "$DMG"), found ${#apps[@]}" >&2
  exit 1
fi
APP="${apps[0]}"

codesign -dv --verbose=2 "$APP"

# --deep --strict is the check that matters, and the one rc1 failed with "code has
# no resources but signature indicates they must be present". A plain
# `codesign -dv` prints a plausible-looking signature for that same broken bundle,
# so it is not a substitute.
codesign --verify --deep --strict --verbose=2 "$APP"

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  echo "APPLE_SIGNING_IDENTITY set - verifying Developer ID signature and notarization staple"
  spctl -a -vv "$APP"
  xcrun stapler validate "$APP"
  # The DMG is what gets double-clicked. Once it carries a Developer ID
  # signature Gatekeeper assesses the image itself on open, and rejects one
  # that is signed but not notarized ("source=Unnotarized Developer ID") -
  # seen on the first real-signing rehearsal, where the .app passed both
  # checks above. This is the assessment run when the image is opened.
  echo "verifying the DMG's own notarization"
  spctl -a -vv -t open --context context:primary-signature "$DMG"
  xcrun stapler validate "$DMG"
fi

echo "macOS signature verified: $(basename "$APP") inside $(basename "$DMG")"
