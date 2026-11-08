#!/usr/bin/env bash
set -euo pipefail

# release-tag.sh - tag an already-bumped tree, with the checks release.sh has.
#
# Usage:
#   ./scripts/release-tag.sh <vX.Y.Z|vX.Y.Z-rcN> [--allow-unannounced] [--dry-run]
#
# There are two release flows (docs/releasing.md). release.sh owns the first:
# bump the four version files, move the changelog, commit. This script owns the
# second - tagging a tree that is already at the right version - which is how
# every release candidate and every release that follows one gets cut.
#
# That second flow used to be three hand-typed git commands outside every guard
# the tooling has, which is how four fixes shipped unannounced between v0.5.0-rc1
# and rc2 (#406). What it checks, none of it new information, all of it
# previously left to the operator remembering:
#
#   - the tag is well-formed and does not already exist
#   - the tree is clean, so the tag means what the commit says
#   - all four version files read the tag's base version, so v0.5.1-rc1 cannot
#     be cut from a tree still saying 0.5.0
#   - the notes that will actually be published are non-empty, and nothing is
#     left under [Unreleased] to ship unannounced
#
# It creates a local tag and stops. Pushing is the irreversible step - it starts
# a paid three-platform build and, for a bare vX.Y.Z, publishes as "Latest" - so
# it stays a deliberate act, printed as the next step rather than done for you.
#
# The version files deliberately stay at the plain X.Y.Z for a candidate: the
# Windows MSI bundler rejects a non-numeric pre-release identifier, so an
# in-tree 0.5.0-rc1 fails the Windows build outright. The candidate lives only
# in the tag.

SCRIPT_DIR="$(dirname "$(realpath "$0")")"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/release-version.sh
. "$SCRIPT_DIR/lib/release-version.sh"

# Compares two dotted X.Y.Z version strings numerically, component by
# component - a plain string compare would rank "0.5.9" above "0.5.10" (and,
# via `sort`, put "0.5.10" ahead of "0.5.9" lexically too - the boundary the
# weekly-supersession scan below is written to never depend on). Bash arrays
# only, no external tools, so it works the same under the BSD userland
# (macOS) this script otherwise avoids GNU flags for.
version_ge() {
  local IFS=.
  local -a av=($1) bv=($2)
  local i x y
  for i in 0 1 2; do
    x="${av[i]:-0}"
    y="${bv[i]:-0}"
    if [ "$x" -gt "$y" ]; then return 0; fi
    if [ "$x" -lt "$y" ]; then return 1; fi
  done
  return 0
}

# Strictly greater, same numeric-component comparison. The weekly-supersession
# warning only fires on THIS - at an equal base the stable release and the
# weekly series converge (see docs/releasing.md, "The weekly version
# invariant"), and the stable correctly supersedes it there, so `version_ge`
# would over-warn on the one case that is not actually a problem.
version_gt() {
  version_ge "$1" "$2" && ! version_ge "$2" "$1"
}

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/release-tag.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT

# --- Parse arguments ---
TAG=""
ALLOW_UNANNOUNCED=false
DRY_RUN=false

for arg in "$@"; do
  case "$arg" in
    --allow-unannounced) ALLOW_UNANNOUNCED=true ;;
    --dry-run) DRY_RUN=true ;;
    -*) echo "Unknown flag: $arg" >&2; exit 1 ;;
    *)
      # Refuse a second tag rather than silently keeping the last one: the two
      # spellings of a near-miss ("v0.5.0 v0.5.1") should not quietly pick one.
      if [ -n "$TAG" ]; then
        echo "Error: more than one tag given ('$TAG' and '$arg'). Tag one at a time." >&2
        exit 1
      fi
      TAG="$arg"
      ;;
  esac
done

if [ -z "$TAG" ]; then
  echo "Usage: $0 <vX.Y.Z|vX.Y.Z-rcN> [--allow-unannounced] [--dry-run]" >&2
  exit 1
fi

# A suffixed tag is a pre-release. The shape accepted here has to stay in step
# with .github/workflows/release.yml, which decides `--prerelease` from the same
# property by matching `*-*` on the tag name: anything this script accepts as a
# candidate must be something that workflow will refuse to mark as "Latest".
if [[ ! "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
  echo "Error: '$TAG' is not a release tag." >&2
  echo "Expected vX.Y.Z for a release, or vX.Y.Z-<suffix> (e.g. v0.5.1-rc1) for a candidate." >&2
  exit 1
fi

# A weekly tag (v0.5.1-weekly.20260817) matches the shape check above too,
# but it is not one this script hand-tags: weekly tags are reserved for the
# automated weekly pipeline (issue #579, lands separately from this script),
# and a hand-typed weekly tag would either collide with, or masquerade as,
# one of those. Refuse it here rather than let it silently succeed as a
# candidate.
case "$TAG" in
  *-weekly.*)
    echo "Error: '$TAG' looks like a weekly tag." >&2
    echo "Weekly tags are reserved for the automated weekly pipeline (#579," >&2
    echo "lands separately) - they are never hand-tagged." >&2
    echo "See docs/releasing.md (Updater manifests) for the weekly-track flow." >&2
    echo "Nothing was tagged." >&2
    exit 1
    ;;
esac

VERSION="${TAG#v}"
BASE_VERSION="${VERSION%%-*}"
PRERELEASE=false
[ "$VERSION" != "$BASE_VERSION" ] && PRERELEASE=true

# --- Pre-flight: everything read-only, before the tag is created ---

if git -C "$PROJECT_ROOT" rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  echo "Error: tag $TAG already exists." >&2
  echo "Candidates are kept, not recycled - a tester's bug report names one." >&2
  echo "Cut the next number instead." >&2
  exit 1
fi

# Stricter than release.sh, which tolerates a dirty CHANGELOG.md because it is
# about to commit it. Here there is no commit: anything uncommitted is by
# definition not in the tree being tagged, so a dirty changelog means the notes
# on disk are not the notes that will publish.
#
# Untracked files count too, and the message has to say so: `git stash` without
# `-u` does not take them, so "commit or stash" alone is advice that does not
# work. An untracked file cannot change what the tag names, but a release cut
# with one sitting there is usually a forgotten `git add`, which can.
DIRT="$(git -C "$PROJECT_ROOT" status --porcelain)"
if [ -n "$DIRT" ]; then
  echo "Error: the working tree is not clean." >&2
  echo "A tag names a commit, so nothing below is in the release:" >&2
  printf '%s\n' "$DIRT" | sed 's/^/  /' >&2
  echo "Commit them (or 'git stash -u' - plain stash leaves '??' files behind)." >&2
  echo "Nothing was tagged." >&2
  exit 1
fi

# The check the manual flow never had. Cutting v0.5.1-rc1 from a tree whose
# manifests still say 0.5.0 builds artifacts named Stalagmight_0.5.0_* and an
# About box reading 0.5.0, from a tag promising otherwise.
if ! version_files_match "$BASE_VERSION" "tag $TAG" "for $TAG"; then
  echo "" >&2
  echo "The version files do not match the tag. Either you meant a different tag," >&2
  echo "or the bump never ran: ./scripts/release.sh $BASE_VERSION --no-edit --no-tag" >&2
  echo "Nothing was tagged." >&2
  exit 1
fi

# --- The notes that will actually be published ---
#
# Rendered by the same script the workflow runs, rather than re-deriving which
# changelog section wins here. That is the whole point: a check built on a second
# model of the selection rule can agree with itself and still disagree with what
# publishes.
#
# CHANGELOG is pinned rather than inherited. release-notes.sh takes
# `${CHANGELOG:-...}` so a caller can point it at a fixture, which is how
# test-release.sh exercises it - but here that override would aim the gate at
# one file while CI publishes from the repository's own, and the stale-date
# advisory below already reads the real one. An exported CHANGELOG would then
# wave through a tag whose actual notes leave entries unannounced.
NOTES_ERR="$SCRATCH/notes.err"
if ! BODY="$(CHANGELOG="$PROJECT_ROOT/CHANGELOG.md" "$SCRIPT_DIR/release-notes.sh" "$TAG" 2>"$NOTES_ERR")"; then
  echo "Error: release-notes.sh failed for $TAG, so the published body is unknown." >&2
  cat "$NOTES_ERR" >&2
  echo "Nothing was tagged." >&2
  exit 1
fi

# release-notes.sh falls back to this exact one-line body when it finds no
# section at all. Publishing it would announce a release by restating its
# version number, so treat it as a missing changelog rather than a valid body.
# -F because the tag's dots are literals here, not wildcards.
if printf '%s\n' "$BODY" | grep -qxF "Release $TAG"; then
  echo "Error: CHANGELOG.md has no notes for $TAG." >&2
  echo "Neither '## [$BASE_VERSION]' nor '## [Unreleased]' has any content, so the" >&2
  echo "published body would be the bare string 'Release $TAG'." >&2
  echo "Write the notes first. Nothing was tagged." >&2
  exit 1
fi

# The gate this script exists for. Once '## [X.Y.Z]' exists - the state every
# candidate after the first is cut from - release-notes.sh stops reading
# [Unreleased] entirely, so entries left there ship in the binary and appear in
# no notes. It has always warned about this; nothing called it before a tag
# existed, and the warning it prints from CI arrives after publication.
#
# Coupled to that script's wording on purpose, so there is one definition of the
# condition rather than two. test-release.sh pins both ends: that release-notes.sh
# still emits this string, and that this script refuses when it does.
if grep -q "will NOT be published" "$NOTES_ERR"; then
  if [ "$ALLOW_UNANNOUNCED" = false ]; then
    cat "$NOTES_ERR" >&2
    echo "Refusing to tag. Fold those entries into '## [$BASE_VERSION]' in CHANGELOG.md," >&2
    echo "commit, and re-run - or pass --allow-unannounced to ship them unmentioned." >&2
    echo "Nothing was tagged." >&2
    exit 1
  fi
  cat "$NOTES_ERR" >&2
  echo "Continuing anyway: --allow-unannounced was passed." >&2
  echo "" >&2
fi

# --- Advisories: worth seeing, not worth blocking on ---

# A final release that was never a candidate is legitimate (a hotfix, a patch
# nobody needs to test first), so this is a note. It is here because the
# opposite mistake - believing rc3 was tested when the tag was never pushed - is
# invisible otherwise.
if [ "$PRERELEASE" = false ]; then
  if [ -z "$(git -C "$PROJECT_ROOT" tag --list "v${BASE_VERSION}-*")" ]; then
    echo "note: no v${BASE_VERSION}-rcN tag exists - this release was never cut as a candidate." >&2
  fi

  # release.sh stamped the section header the day the version was bumped, which
  # for anything that went through candidates is well before the day it ships.
  # The date is published verbatim in the notes.
  # A literal prefix via index(), not a regex. awk re-processes backslash escapes
  # in a -v assignment, so a grep/sed-escaped version arrives with its dots bare
  # and "## [0X5Y0] - ..." would match - the escaping defeated by the tool it was
  # handed to. release.sh writes exactly this header shape; anything else simply
  # yields no date and no advisory.
  SECTION_DATE="$(awk -v hdr="## [$BASE_VERSION] - " '
    index($0, hdr) == 1 { print $NF; exit }
  ' "$PROJECT_ROOT/CHANGELOG.md")"
  TODAY="$(date +%Y-%m-%d)"
  if [ -n "$SECTION_DATE" ] && [ "$SECTION_DATE" != "$TODAY" ]; then
    echo "note: CHANGELOG.md dates $BASE_VERSION as $SECTION_DATE, but today is $TODAY." >&2
    echo "      That date is published as-is; it is stamped when the version is bumped," >&2
    echo "      not when it ships. Update it first if it should read today." >&2
  fi

  # Weekly-track supersession warning (issue #579). The updater's weekly
  # manifest only ever moves a track forward, so a weekly tag whose base is
  # STRICTLY GREATER than this stable's version means weekly-track users on
  # that build (or later in the same series) will never be offered this
  # stable release - the weekly manifest already points past it. An EQUAL
  # base is not a problem: that is the convergence point the weekly series
  # was built toward (see docs/releasing.md, "The weekly version invariant"),
  # and the stable correctly supersedes it there. This is advisory, not a
  # refusal: a maintainer may already know and be fine with it (e.g. a patch
  # cut behind a weekly build that shipped nothing users need).
  #
  # Scans every `v*-weekly.*` tag and keeps the numerically HIGHEST base
  # found, rather than the first `sort`-ordered match: `sort` orders
  # lexically, which puts "0.5.10" ahead of "0.5.9" for the wrong reason (a
  # leading "1" sorts before "9"), so stopping at the first hit can both
  # false-positive on an equal-base tag that happens to sort first and miss a
  # genuinely higher one further down the list.
  HIGHEST_WEEKLY=""
  HIGHEST_WEEKLY_BASE=""
  while IFS= read -r wtag; do
    [ -z "$wtag" ] && continue
    wbase="${wtag#v}"
    wbase="${wbase%%-weekly.*}"
    if [ -z "$HIGHEST_WEEKLY_BASE" ] || version_gt "$wbase" "$HIGHEST_WEEKLY_BASE"; then
      HIGHEST_WEEKLY_BASE="$wbase"
      HIGHEST_WEEKLY="$wtag"
    fi
  done < <(git -C "$PROJECT_ROOT" tag --list 'v*-weekly.*')
  if [ -n "$HIGHEST_WEEKLY" ] && version_gt "$HIGHEST_WEEKLY_BASE" "$BASE_VERSION"; then
    echo "note: weekly tag $HIGHEST_WEEKLY already carries a base version > $BASE_VERSION." >&2
    echo "      Weekly-track users on or past that build will not be offered $TAG:" >&2
    echo "      the weekly manifest only moves forward, and this stable release does" >&2
    echo "      not supersede it. Continuing is fine if that is expected." >&2
  fi
fi

# Pushing a tag pushes the commit with it, so this is not broken - but it would
# publish a release built from a commit that is on nobody else's master.
if git -C "$PROJECT_ROOT" rev-parse -q --verify refs/remotes/origin/master >/dev/null 2>&1; then
  if ! git -C "$PROJECT_ROOT" merge-base --is-ancestor HEAD refs/remotes/origin/master 2>/dev/null; then
    echo "note: HEAD is not an ancestor of origin/master (as last fetched)." >&2
    echo "      Push the branch before the tag, or the release ships a commit master does not have." >&2
  fi
fi

# --- Show the operator what they are about to publish ---
echo "Tag:      $TAG"
echo "Version:  $BASE_VERSION (in package.json, tauri.conf.json, Cargo.toml, Cargo.lock)"
echo "Commit:   $(git -C "$PROJECT_ROOT" log -1 --format='%h %s')"
if [ "$PRERELEASE" = true ]; then
  echo "Kind:     pre-release (published as a candidate, never as Latest)"
else
  echo "Kind:     release (published as Latest)"
fi
echo ""
echo "--- release body ---"
printf '%s\n' "$BODY"
echo "--- end release body ---"
echo ""

if [ "$DRY_RUN" = true ]; then
  echo "[dry-run] Would tag: $TAG"
  exit 0
fi

if [ "$PRERELEASE" = true ]; then
  git -C "$PROJECT_ROOT" tag -a "$TAG" -m "Release candidate $TAG"
else
  git -C "$PROJECT_ROOT" tag -a "$TAG" -m "Release $TAG"
fi

echo "Created $TAG locally. Nothing is published until it is pushed."
echo ""
echo "Next step:"
echo "  git push origin $TAG"
