#!/usr/bin/env bash
set -euo pipefail

# release.sh - bump version, update changelog, commit and tag
#
# Usage:
#   ./scripts/release.sh <major|minor|patch|X.Y.Z> [--no-edit] [--dry-run]
#
# The changelog is driven by the ## [Unreleased] section in CHANGELOG.md.
# Write your release notes there BEFORE running this script. The script
# moves that content under the new version header.
#
# If the [Unreleased] section is empty, the script aborts (unless --no-edit
# is omitted, in which case it opens $EDITOR so you can write notes).

SCRIPT_DIR="$(dirname "$(realpath "$0")")"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# lock_stanza_count, lock_version, require_one_match, version_pattern,
# version_files_match - shared with release-tag.sh, which has to ask the same
# question about the same four files.
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/release-version.sh
. "$SCRIPT_DIR/lib/release-version.sh"

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/release.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT

# Rewrite a file through sed without `sed -i`, which is not portable: GNU takes
# no argument, BSD/macOS requires one. Redirecting into the original preserves
# its mode and inode.
replace_in_file() {
  local expr="$1" file="$2" tmp="$SCRATCH/rewrite"
  sed "$expr" "$file" > "$tmp"
  # A rewrite that empties the file means sed did something unintended; better to
  # stop than to truncate a manifest.
  if [ ! -s "$tmp" ]; then
    echo "Error: rewriting $file produced an empty file. Aborting." >&2
    exit 1
  fi
  cat "$tmp" > "$file"
}

# Rewrite the Cargo.lock version line, and fail rather than write if it is not found exactly
# once. Cargo would fix the version itself on the next build, but only by leaving
# an unexplained dirty Cargo.lock in a tree the release just reported as clean -
# and a hard error for anyone who does build with `--locked`.
#
# Why hand-rolled awk rather than `cargo update -p stalagmight --offline`, which
# would let Cargo own its own format: this script deliberately has no toolchain
# dependency (no cargo, no node, no network), so it works on a machine set up only
# to cut a release, and it stays a pure text transform that `test-release.sh` can
# exercise in a throwaway clone in milliseconds. The cost is a second model of one
# line of the lock format, bounded by `END { if (hits != 1) exit 3 }` writing to a
# temp file - a shape it does not recognise aborts with the manifest untouched
# rather than guessing. Revisit if the lock ever carries this crate's version in
# more than one place.
rewrite_lock_version() {
  local new="$1" file="$PROJECT_ROOT/src-tauri/Cargo.lock" tmp="$SCRATCH/lockrewrite"
  awk -v new="$new" '
    /^name = "stalagmight"[[:space:]]*$/ {
      print
      if ((getline nxt) > 0) {
        if (nxt ~ /^version = "/) {
          # Keep whatever line ending the file already uses, so rewriting one line
          # of a CRLF checkout does not leave a single stray LF behind.
          print "version = \"" new "\"" (nxt ~ /\r$/ ? "\r" : "")
          hits++
        } else { print nxt }
      }
      next
    }
    { print }
    END { if (hits != 1) exit 3 }
  ' "$file" > "$tmp" || {
    echo "Error: src-tauri/Cargo.lock has no single stalagmight version line. Aborting." >&2
    return 1
  }
  if [ ! -s "$tmp" ]; then
    echo "Error: rewriting Cargo.lock produced an empty file. Aborting." >&2
    return 1
  fi
  cat "$tmp" > "$file"
}

# --- Parse arguments ---
VERSION_ARG=""
NO_EDIT=false
DRY_RUN=false
NO_TAG=false

for arg in "$@"; do
  case "$arg" in
    --no-edit) NO_EDIT=true ;;
    --dry-run) DRY_RUN=true ;;
    --no-tag) NO_TAG=true ;;
    -*) echo "Unknown flag: $arg" >&2; exit 1 ;;
    *) VERSION_ARG="$arg" ;;
  esac
done

if [ -z "$VERSION_ARG" ]; then
  echo "Usage: $0 <major|minor|patch|X.Y.Z> [--no-edit] [--dry-run]" >&2
  exit 1
fi

# --- Read current version from package.json ---
# POSIX sed rather than `grep -oP`, which BSD/macOS grep does not support.
CURRENT_VERSION=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
  "$PROJECT_ROOT/package.json" | head -1)
if [ -z "$CURRENT_VERSION" ]; then
  echo "Error: could not read \"version\" from package.json." >&2
  exit 1
fi
echo "Current version: $CURRENT_VERSION"

# --- Compute new version ---
IFS='.' read -r CUR_MAJOR CUR_MINOR CUR_PATCH <<< "$CURRENT_VERSION"

case "$VERSION_ARG" in
  major) NEW_VERSION="$((CUR_MAJOR + 1)).0.0" ;;
  minor) NEW_VERSION="$CUR_MAJOR.$((CUR_MINOR + 1)).0" ;;
  patch) NEW_VERSION="$CUR_MAJOR.$CUR_MINOR.$((CUR_PATCH + 1))" ;;
  *)
    # Validate explicit X.Y.Z
    if [[ ! "$VERSION_ARG" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "Error: version must be 'major', 'minor', 'patch', or X.Y.Z" >&2
      exit 1
    fi
    NEW_VERSION="$VERSION_ARG"
    ;;
esac

echo "New version: $NEW_VERSION"

if [ "$NEW_VERSION" = "$CURRENT_VERSION" ]; then
  echo "Error: $NEW_VERSION is already the current version." >&2
  echo "Every substitution below would be a no-op that still looks like it worked," >&2
  echo "producing a release commit that bumps nothing." >&2
  exit 1
fi

if git -C "$PROJECT_ROOT" rev-parse -q --verify "refs/tags/v$NEW_VERSION" >/dev/null 2>&1; then
  echo "Error: tag v$NEW_VERSION already exists." >&2
  exit 1
fi

# Version strings are substituted into grep/sed patterns, where an unescaped dot
# is a wildcard - "0.2.0" would match "0X2Y0".
CURRENT_RE=$(version_pattern "$CURRENT_VERSION")
NEW_RE=$(version_pattern "$NEW_VERSION")

NEW_JSON="\"version\": \"$NEW_RE\""
NEW_TOML="^version = \"$NEW_RE\""

# --- Pre-flight: prove all four files have the shape the rewrites assume ---
# Checking before writing rather than after is what keeps a format drift in the
# fourth file from leaving the first three already bumped on disk. These checks are
# read-only, so they run BEFORE the --dry-run exit too: a drifted Cargo.lock is the
# one condition this script most needs to report, and a dry run that printed a
# confident "would update" for it while the real run refuses would be worse than
# useless.
if ! version_files_match "$CURRENT_VERSION" "package.json" "before the rewrite"; then
  echo "Nothing was modified." >&2
  exit 1
fi

if [ "$DRY_RUN" = true ]; then
  echo ""
  echo "[dry-run] Would update:"
  echo "  package.json:         $CURRENT_VERSION -> $NEW_VERSION"
  echo "  src-tauri/tauri.conf.json: $CURRENT_VERSION -> $NEW_VERSION"
  echo "  src-tauri/Cargo.toml: $CURRENT_VERSION -> $NEW_VERSION"
  echo "  src-tauri/Cargo.lock: $CURRENT_VERSION -> $NEW_VERSION"
  echo "  CHANGELOG.md:         move [Unreleased] content under v$NEW_VERSION"
  echo "[dry-run] Would commit: chore: release v$NEW_VERSION"
  echo "[dry-run] Would tag:    v$NEW_VERSION"
  exit 0
fi

# --- Check for uncommitted changes (allow a modified CHANGELOG.md) ---
# `git status --porcelain`, as release-tag.sh does: it lists untracked files,
# which `git diff --name-only HEAD` never did - and a release cut with one
# sitting there is usually a forgotten `git add`. A git FAILURE stops here: the
# old `2>/dev/null | grep -v ... || true` turned it into "nothing changed" and
# went on to rewrite the version files (#576 E18(f)). CHANGELOG.md is the one
# exemption, because this script is about to commit it.
if ! DIRT="$(git -C "$PROJECT_ROOT" status --porcelain)"; then
  echo "Error: git status failed in $PROJECT_ROOT, so the tree cannot be checked for uncommitted changes." >&2
  echo "Nothing was changed." >&2
  exit 1
fi
CHANGED="$(printf '%s\n' "$DIRT" | awk 'NF && substr($0, 4) != "CHANGELOG.md"')"
if [ -n "$CHANGED" ]; then
  echo "Error: the working tree has changes other than CHANGELOG.md:" >&2
  printf '%s\n' "$CHANGED" | sed 's/^/  /' >&2
  echo "Commit them (or 'git stash -u' - plain stash leaves '??' files behind)." >&2
  exit 1
fi

# --- Read [Unreleased] section from CHANGELOG.md ---
CHANGELOG="$PROJECT_ROOT/CHANGELOG.md"
UNRELEASED=$(awk '
  /^## \[Unreleased\]/ { found=1; next }
  found && /^## \[/ { exit }
  found { print }
' "$CHANGELOG" | sed '/^$/d')

if [ -z "$UNRELEASED" ] && [ "$NO_EDIT" = true ]; then
  echo "Error: ## [Unreleased] section in CHANGELOG.md is empty." >&2
  echo "Write release notes there before running with --no-edit." >&2
  exit 1
fi

if [ -z "$UNRELEASED" ] && [ "$NO_EDIT" = false ]; then
  echo "## [Unreleased] section is empty. Opening editor..."
  EDITOR="${EDITOR:-${VISUAL:-vi}}"
  "$EDITOR" "$CHANGELOG"
  # Re-read after edit
  UNRELEASED=$(awk '
    /^## \[Unreleased\]/ { found=1; next }
    found && /^## \[/ { exit }
    found { print }
  ' "$CHANGELOG" | sed '/^$/d')
  if [ -z "$UNRELEASED" ]; then
    echo "Error: [Unreleased] section is still empty. Aborting." >&2
    exit 1
  fi
fi

# --- Update package.json ---
replace_in_file "s/\"version\": \"$CURRENT_RE\"/\"version\": \"$NEW_VERSION\"/" \
  "$PROJECT_ROOT/package.json"

# --- Update tauri.conf.json ---
replace_in_file "s/\"version\": \"$CURRENT_RE\"/\"version\": \"$NEW_VERSION\"/" \
  "$PROJECT_ROOT/src-tauri/tauri.conf.json"

# --- Update Cargo.toml (only the package version line) ---
replace_in_file "s/^version = \"$CURRENT_RE\"/version = \"$NEW_VERSION\"/" \
  "$PROJECT_ROOT/src-tauri/Cargo.toml"

# --- Update Cargo.lock (only this crate's stanza) ---
if ! rewrite_lock_version "$NEW_VERSION"; then
  git -C "$PROJECT_ROOT" checkout -- \
    package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
  echo "Reverted the version files. No commit or tag was made." >&2
  exit 1
fi

# --- Confirm every bump landed, on exactly one line ---
if ! require_one_match "package.json"              "$NEW_JSON" "after the rewrite" \
  || ! require_one_match "src-tauri/tauri.conf.json" "$NEW_JSON" "after the rewrite" \
  || ! require_one_match "src-tauri/Cargo.toml"      "$NEW_TOML" "after the rewrite" \
  || [ "$(lock_version)" != "$NEW_VERSION" ]; then
  git -C "$PROJECT_ROOT" checkout -- \
    package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
  echo "Reverted the version files. No commit or tag was made." >&2
  exit 1
fi

# --- Update CHANGELOG.md: move [Unreleased] content under new version header ---
TODAY=$(date +%Y-%m-%d)

awk -v ver="$NEW_VERSION" -v date="$TODAY" '
  /^## \[Unreleased\]/ {
    print "## [Unreleased]"
    printf "\n## [%s] - %s\n", ver, date
    # Skip blank lines between [Unreleased] and content, content is already there
    next
  }
  { print }
' "$CHANGELOG" > "$CHANGELOG.tmp" && mv "$CHANGELOG.tmp" "$CHANGELOG"

# awk exits 0 whether or not it matched, so a missed [Unreleased] header would
# copy the file through untouched and still look like success. Unreachable today
# - the emptiness check above keys off the same pattern and already aborts when
# it does not match - so this is belt-and-braces in case that check is ever
# relaxed. CHANGELOG.md is deliberately NOT reverted: it can hold hand-written
# release notes that are not committed yet.
if ! grep -q "^## \[$NEW_RE\]" "$CHANGELOG"; then
  echo "Error: CHANGELOG.md has no '## [$NEW_VERSION]' section after the rewrite." >&2
  echo "The '## [Unreleased]' header was probably not found." >&2
  git -C "$PROJECT_ROOT" checkout -- \
    package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
  echo "Reverted the version files, left CHANGELOG.md alone. No commit or tag was made." >&2
  exit 1
fi

# --- Open editor for final review ---
#
# Under `set -e` a bare invocation here would abort the script between the last
# rewrite and `git add`, stranding four bumped manifests with no commit, no tag and
# no message - and the clean-tree check above then refuses the obvious re-run, so
# the operator has to hand-revert. Vim's `:cq` and a misconfigured $EDITOR both
# exit non-zero, so this is reachable in normal use. Revert the version files the
# same way every other failure path does, and say what happened.
if [ "$NO_EDIT" = false ]; then
  EDITOR="${EDITOR:-${VISUAL:-vi}}"
  if ! "$EDITOR" "$CHANGELOG"; then
    git -C "$PROJECT_ROOT" checkout -- \
      package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
    echo "" >&2
    echo "Error: \$EDITOR ($EDITOR) exited non-zero. No commit or tag was made." >&2
    echo "Reverted the version files. CHANGELOG.md is left as-is, with the" >&2
    echo "[Unreleased] content already moved under v$NEW_VERSION - review it before" >&2
    echo "re-running, or 'git checkout -- CHANGELOG.md' to start over." >&2
    exit 1
  fi
fi

# --- Commit and tag ---
git -C "$PROJECT_ROOT" add \
  package.json \
  src-tauri/tauri.conf.json \
  src-tauri/Cargo.toml \
  src-tauri/Cargo.lock \
  CHANGELOG.md

git -C "$PROJECT_ROOT" commit -m "chore: release v$NEW_VERSION"

if [ "$NO_TAG" = true ]; then
  echo ""
  echo "Prepared v$NEW_VERSION (no tag created)"
  echo ""
  # Through release-tag.sh, not `git tag`: it is the half of the release that
  # this script's checks do not cover, and a bare tag command gets none of them.
  echo "Next steps:"
  echo "  git push"
  echo "  ./scripts/release-tag.sh v$NEW_VERSION-rc1   # a candidate, then rc2, rc3, ..."
  echo "  ./scripts/release-tag.sh v$NEW_VERSION       # the release"
  echo "  git push origin <the tag it created>"
  exit 0
fi

git -C "$PROJECT_ROOT" tag -a "v$NEW_VERSION" -m "Release v$NEW_VERSION"

echo ""
echo "Released v$NEW_VERSION"
echo ""
echo "Next steps:"
echo "  git push && git push --tags"
