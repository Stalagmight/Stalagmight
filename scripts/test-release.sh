#!/usr/bin/env bash
set -euo pipefail

# test-release.sh - exercise scripts/release.sh against a throwaway clone.
#
# release.sh rewrites version strings across three files and then commits and
# tags. Nothing else in the repo verifies it, and it only runs for real when
# someone is cutting a release - the worst moment to discover a bug in it. Each
# case below drives the real script and asserts on the resulting tree.
#
# Usage: ./scripts/test-release.sh

SCRIPT_DIR="$(dirname "$(realpath "$0")")"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/release-test.XXXXXX")"
# `set -e` already aborts on a failed mktemp, but everything below feeds `$WORK`
# to `rm -rf`, so assert it non-empty rather than trusting that one guard - an
# empty value here turns the cleanup trap into `rm -rf ""` at the repo root.
[ -n "$WORK" ] && [ -d "$WORK" ] || { echo "mktemp -d produced no work dir" >&2; exit 1; }
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0

fail() {
  echo "  FAIL: $1" >&2
  FAIL=$((FAIL + 1))
}

pass() {
  echo "  ok: $1"
  PASS=$((PASS + 1))
}

# assert "label" cmd...  - the command must succeed
# refute "label" cmd...  - the command must fail
# Spelled out rather than `cmd && pass || fail`, which double-reports if `pass`
# ever returns non-zero.
assert() {
  local msg="$1"
  shift
  if "$@"; then pass "$msg"; else fail "$msg"; fi
}

refute() {
  local msg="$1"
  shift
  if "$@"; then fail "$msg"; else pass "$msg"; fi
}

tree_clean() {
  [ -z "$(git -C "$1" status --porcelain)" ]
}

has_tag() {
  git -C "$1" rev-parse -q --verify "refs/tags/$2" >/dev/null
}

# A fresh clone per case, so one case's mutations can't reach the next.
fresh_clone() {
  local dir="$WORK/$1" sha
  sha="$(git -C "$PROJECT_ROOT" rev-parse HEAD)"
  rm -rf "$dir"
  git clone --quiet --local --no-hardlinks "$PROJECT_ROOT" "$dir"
  # Give the clone its own identity. Several commits land here that this script
  # does not make itself - release.sh's, and the fixture commits below - so a
  # machine with no global user.name/user.email (a fresh runner or container)
  # would fail them with "Author identity unknown".
  git -C "$dir" config user.email "test@example.invalid"
  git -C "$dir" config user.name "release test"
  # Detach at the exact commit under test rather than trusting which branch the
  # clone checked out, which works the same on a branch and in CI's detached HEAD.
  git -C "$dir" checkout --quiet --detach "$sha"
  # Drop the inherited tags, so every case's tag state is one the case built.
  # A clone carries the real history's tags, and the release-tag.sh cases derive
  # their tag from the clone's OWN current version - which is exactly the version
  # whose candidates the repo already has. Without this, those cases refuse with
  # "tag already exists" and pass for a reason they are not testing, or abort the
  # run outright when a case tries to create one.
  local tags
  tags="$(git -C "$dir" tag -l)"
  if [ -n "$tags" ]; then
    # Deliberate word splitting: tag names carry no spaces.
    # shellcheck disable=SC2086
    git -C "$dir" tag -d $tags >/dev/null
  fi
  # Test the scripts as they exist on disk, not as last committed, so this is
  # useful while iterating on them too. Committed in the clone so the cases can
  # assert on a clean tree without seeing these copies as modifications.
  #
  # All four, not just release.sh: the two entry points now share
  # lib/release-version.sh, and release-tag.sh shells out to release-notes.sh for
  # the body it gates on. Copying only some of them would test an on-disk script
  # against its committed collaborators - which passes right up until the change
  # under test is in the part that was not copied.
  mkdir -p "$dir/scripts/lib"
  cp "$PROJECT_ROOT/scripts/release.sh"            "$dir/scripts/release.sh"
  cp "$PROJECT_ROOT/scripts/release-tag.sh"        "$dir/scripts/release-tag.sh"
  cp "$PROJECT_ROOT/scripts/release-notes.sh"      "$dir/scripts/release-notes.sh"
  cp "$PROJECT_ROOT/scripts/lib/release-version.sh" "$dir/scripts/lib/release-version.sh"
  git -C "$dir" add scripts/release.sh scripts/release-tag.sh \
    scripts/release-notes.sh scripts/lib/release-version.sh
  # Pin the changelog these cases run against instead of inheriting the live one.
  # release.sh refuses an empty `## [Unreleased]` - case 5 asserts that refusal is
  # correct - so cutting a release, which empties that section by moving it under
  # the new version, turned cases 1, 6 and 9 red on master for a reason none of
  # them is about. A fixture makes every outcome attributable to the case's own
  # name; case 5 overwrites it with an empty section on purpose.
  # Non-empty matters for case 7 as much as for the happy paths: with notes
  # already present, release.sh skips the write-them-now editor at the top and
  # reaches the post-rewrite one, which is the editor whose failure case 7 exists
  # to test. Against an empty section it aborts at the first editor instead, with
  # no version file ever written, and case 7 passes without testing anything.
  printf '%s\n' \
    '# Changelog' '' \
    '## [Unreleased]' '' '### Fixed' '' '- a fixture entry, so the section is not empty' '' \
    '## [0.1.0] - 2026-01-01' '' '- something old' \
    > "$dir/CHANGELOG.md"
  git -C "$dir" add CHANGELOG.md
  git -C "$dir" commit --quiet --allow-empty -m "test: script under test"
  echo "$dir"
}

version_of() {
  sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$1/package.json" | head -1
}

# Cargo.lock has a `version = "..."` line per dependency, so read only the one
# belonging to this crate: the line right after its `name` line.
# Deliberately a SECOND implementation of lib/release-version.sh's `lock_version`,
# not a call to it: a test that reads the value through the same code it is
# checking would confirm that code agrees with itself. Includes the same CRLF
# tolerance - without it this reads empty on a CRLF checkout and every assertion
# built on it fails for the wrong reason, or worse, passes vacuously.
lock_version_of() {
  awk '
    /^name = "stalagmight"[[:space:]]*$/ {
      if ((getline nxt) > 0) {
        sub(/\r$/, "", nxt)
        if (nxt ~ /^version = "/) {
          gsub(/^version = "|"$/, "", nxt)
          print nxt
        }
      }
    }
  ' "$1/src-tauri/Cargo.lock"
}

# Rewrite that line to an arbitrary value, to fixture a drifted lock.
# The stanza pattern carries the same `[[:space:]]*$` CRLF tolerance as the real
# helper: without it this matches nothing on a CRLF checkout, so the case that
# calls it fixtures no drift and then asserts a refusal that cannot come.
set_lock_version() {
  awk -v v="$2" '
    /^name = "stalagmight"[[:space:]]*$/ { print; getline; print "version = \"" v "\""; next }
    { print }
  ' "$1/src-tauri/Cargo.lock" > "$1/.locktmp"
  mv "$1/.locktmp" "$1/src-tauri/Cargo.lock"
}

echo "== case 1: a normal bump rewrites all four files, commits and tags =="
D=$(fresh_clone case1)
if "$D/scripts/release.sh" 9.9.9 --no-edit >/dev/null 2>&1; then
  assert "package.json bumped"       test "$(version_of "$D")" = "9.9.9"
  assert "tauri.conf.json bumped"    grep -q '"version": "9.9.9"' "$D/src-tauri/tauri.conf.json"
  assert "Cargo.toml bumped"         grep -q '^version = "9.9.9"' "$D/src-tauri/Cargo.toml"
  # Cargo.lock is the fourth version file. Before #352 it was left behind, which
  # nothing failed on: the next cargo invocation rewrote it in place, so the only
  # symptom was an unexplained dirty lock in a tree the release called clean.
  assert "Cargo.lock bumped"         test "$(lock_version_of "$D")" = "9.9.9"
  assert "Cargo.lock is in the release commit" \
    bash -c 'git -C "$1" show --name-only --format= HEAD | grep -qx "src-tauri/Cargo.lock"' _ "$D"
  assert "changelog section created" grep -q '^## \[9\.9\.9\]' "$D/CHANGELOG.md"
  assert "tag created"               has_tag "$D" v9.9.9
  assert "tree clean after release"  tree_clean "$D"
else
  fail "release.sh exited non-zero on the happy path"
fi

echo "== case 2: re-releasing the current version is refused =="
D=$(fresh_clone case2)
CUR=$(version_of "$D")
if "$D/scripts/release.sh" "$CUR" --no-edit >/dev/null 2>&1; then
  fail "accepted a no-op bump to the current version"
else
  pass "refused a no-op bump"
  assert "tree untouched" tree_clean "$D"
fi

echo "== case 3: an existing tag is refused =="
D=$(fresh_clone case3)
git -C "$D" tag v8.8.8
if "$D/scripts/release.sh" 8.8.8 --no-edit >/dev/null 2>&1; then
  fail "accepted a version whose tag already exists"
else
  pass "refused an existing tag"
  assert "tree untouched" tree_clean "$D"
fi

echo "== case 4: a drifted version format aborts before ANY file is written =="
# This is the case that used to leave two of three files bumped on disk.
D=$(fresh_clone case4)
sed 's/^version = /version    = /' "$D/src-tauri/Cargo.toml" > "$D/ct" && mv "$D/ct" "$D/src-tauri/Cargo.toml"
git -C "$D" commit --quiet -am "perturb the Cargo.toml version format"
if "$D/scripts/release.sh" 7.7.7 --no-edit >/dev/null 2>&1; then
  fail "accepted a drifted version format"
else
  pass "refused a drifted version format"
  refute "package.json left untouched" grep -q '"version": "7.7.7"' "$D/package.json"
  assert "tree clean, nothing to hand-revert" tree_clean "$D"
fi

echo "== case 4b: a Cargo.lock already out of step aborts before ANY file is written =="
# The lock drifting behind is the #352 bug. If it is ALREADY wrong when a release
# starts, bumping it to the new version would paper over that instead of pointing
# at it, so the pre-flight refuses rather than guessing which one is right.
D=$(fresh_clone case4b)
set_lock_version "$D" 0.0.1
git -C "$D" commit --quiet -am "desync Cargo.lock from Cargo.toml"
if "$D/scripts/release.sh" 7.7.7 --no-edit >/dev/null 2>&1; then
  fail "accepted a Cargo.lock that disagreed with package.json"
else
  pass "refused a Cargo.lock that disagreed with package.json"
  refute "package.json left untouched" grep -q '"version": "7.7.7"' "$D/package.json"
  assert "lock left as found"          test "$(lock_version_of "$D")" = "0.0.1"
  assert "tree clean, nothing to hand-revert" tree_clean "$D"
fi

echo "== case 5: an empty [Unreleased] section aborts before anything is written =="
# Guards against shipping a release whose notes are just "Release vX.Y.Z": the
# workflow falls back to that bare body when it finds no changelog section.
D=$(fresh_clone case5)
printf '# Changelog\n\n## [Unreleased]\n\n## [0.1.0] - 2026-01-01\n\n- something old\n' \
  > "$D/CHANGELOG.md"
git -C "$D" commit --quiet -am "empty the Unreleased section"
if "$D/scripts/release.sh" 6.6.6 --no-edit >/dev/null 2>&1; then
  fail "accepted an empty [Unreleased] section"
else
  pass "refused an empty [Unreleased] section"
  refute "version files untouched" grep -q '"version": "6.6.6"' "$D/package.json"
  refute "no tag created" has_tag "$D" v6.6.6
fi

echo "== case 6: --no-tag bumps and commits but creates no tag =="
D=$(fresh_clone case6)
if "$D/scripts/release.sh" 5.5.5 --no-edit --no-tag >/dev/null 2>&1; then
  assert "package.json bumped"      test "$(version_of "$D")" = "5.5.5"
  assert "changelog moved"          grep -q '^## \[5\.5\.5\]' "$D/CHANGELOG.md"
  assert "committed, tree clean"    tree_clean "$D"
  refute "no tag created"           has_tag "$D" v5.5.5
else
  fail "release.sh --no-tag exited non-zero"
fi

echo "== case 7: a failing \$EDITOR reverts the version files =="
# Without this, `set -e` aborted between the last rewrite and `git add`, leaving
# four bumped manifests, no commit, no tag and no message - and the clean-tree
# check then refuses the obvious re-run. `:cq` and a misconfigured $EDITOR both
# exit non-zero, so this is a normal-use path, not a contrived one.
D=$(fresh_clone case7)
if EDITOR=false "$D/scripts/release.sh" 4.4.4 >/dev/null 2>&1; then
  fail "release.sh ignored a failing \$EDITOR"
else
  pass "refused after \$EDITOR failed"
  refute "package.json reverted"  grep -q '"version": "4\.4\.4"' "$D/package.json"
  assert "Cargo.lock reverted"    test "$(lock_version_of "$D")" != "4.4.4"
  refute "no commit was made"     grep -q '4\.4\.4' <(git -C "$D" log -1 --format=%s)
  refute "no tag created"         has_tag "$D" v4.4.4
  # CHANGELOG.md is deliberately left rewritten (the operator's notes are in it),
  # and the clean-tree check tolerates exactly that one file, so a re-run is
  # possible without hand-reverting anything.
  #
  # This next one is the load-bearing half: release.sh moves the [Unreleased]
  # content under the new header BEFORE it opens the post-rewrite editor, so a
  # `## [4.4.4]` section proves the run actually reached that editor. Without it
  # the porcelain assertion below passes on a COMPLETELY CLEAN tree - `printf '' |
  # grep -cv ...` prints 0 - so an abort at any earlier pre-flight would look
  # identical to the revert this case exists to test.
  assert "the changelog was rewritten, so the editor was reached" \
    grep -q '^## \[4\.4\.4\]' "$D/CHANGELOG.md"
  assert "only CHANGELOG.md left dirty" \
    bash -c '[ "$(git -C "$1" status --porcelain | grep -cv "^ M CHANGELOG.md$")" -eq 0 ]' _ "$D"
fi

echo "== case 8: --dry-run refuses a drifted Cargo.lock instead of promising a bump =="
# The pre-flights are read-only, so they must run before the dry-run exit. A dry
# run that printed a confident "would update" for the one condition the real run
# rejects is worse than no dry run.
D=$(fresh_clone case8)
set_lock_version "$D" 0.0.1
git -C "$D" commit --quiet -am "desync Cargo.lock"
if out=$("$D/scripts/release.sh" 3.3.3 --no-edit --dry-run 2>&1); then
  fail "--dry-run accepted a drifted Cargo.lock"
else
  pass "--dry-run refused a drifted Cargo.lock"
  refute "did not claim it would bump the lock" \
    bash -c 'printf "%s" "$1" | grep -q "Cargo.lock: 0.0.1 ->"' _ "$out"
  assert "tree untouched by the dry run" tree_clean "$D"
fi

echo "== case 9: a CRLF Cargo.lock still releases =="
# The stanza pattern is the script's only end-anchored version check, so a
# core.autocrlf=true checkout used to abort every release - and blame the version
# value rather than the line endings.
D=$(fresh_clone case9)
python3 - "$D/src-tauri/Cargo.lock" <<'PY'
import sys
p = sys.argv[1]
with open(p, 'rb') as f:
    data = f.read()
with open(p, 'wb') as f:
    f.write(data.replace(b'\n', b'\r\n'))
PY
git -C "$D" -c core.autocrlf=false commit --quiet -am "CRLF the lock"
if "$D/scripts/release.sh" 2.2.2 --no-edit --no-tag >/dev/null 2>&1; then
  assert "lock bumped despite CRLF" test "$(lock_version_of "$D")" = "2.2.2"
  assert "line endings preserved" \
    bash -c 'python3 -c "
import sys
d = open(sys.argv[1], \"rb\").read()
sys.exit(0 if d.count(b\"\\n\") == d.count(b\"\\r\\n\") else 1)
" "$1/src-tauri/Cargo.lock"' _ "$D"
else
  fail "release.sh aborted on a CRLF Cargo.lock"
fi

echo "== case 10: a missing stanza reports a SHAPE error, not a value mismatch =="
D=$(fresh_clone case10)
grep -v '^name = "stalagmight"$' "$D/src-tauri/Cargo.lock" > "$D/.lock.tmp"
mv "$D/.lock.tmp" "$D/src-tauri/Cargo.lock"
git -C "$D" commit --quiet -am "drop the stalagmight stanza"
if out=$("$D/scripts/release.sh" 1.1.1 --no-edit 2>&1); then
  fail "accepted a Cargo.lock with no stalagmight stanza"
else
  pass "refused a Cargo.lock with no stalagmight stanza"
  assert "message names the shape problem" \
    bash -c 'printf "%s" "$1" | grep -q "shape is not what the rewrite assumes"' _ "$out"
  assert "tree untouched" tree_clean "$D"
fi

# --- release-tag.sh: the second flow, tagging a tree that is already bumped ---
#
# Every case below was previously "the operator remembers", which is how four
# fixes shipped unannounced between v0.5.0-rc1 and rc2 (#406). A refusal case
# asserts three things every time: it failed, it said why, and it left no tag -
# the last because a script that refuses loudly and tags anyway is the worst
# outcome available.

echo "== case 11: bump then tag a candidate, the two-script flow end to end =="
D=$(fresh_clone case11)
if "$D/scripts/release.sh" 5.5.5 --no-edit --no-tag >/dev/null 2>&1 \
  && out=$("$D/scripts/release-tag.sh" v5.5.5-rc1 2>&1); then
  assert "candidate tag created"  has_tag "$D" v5.5.5-rc1
  # Annotated, not lightweight: release.sh annotates, and `git describe` and the
  # release UI both treat the two differently.
  assert "the tag is annotated" \
    bash -c '[ "$(git -C "$1" cat-file -t v5.5.5-rc1)" = tag ]' _ "$D"
  assert "the body is shown before tagging" \
    bash -c 'printf "%s" "$1" | grep -q "fixture entry"' _ "$out"
  assert "the preview carries the candidate banner" \
    bash -c 'printf "%s" "$1" | grep -q "Release candidate for 5.5.5"' _ "$out"
  # Pushing starts a paid three-platform build and, for a bare vX.Y.Z, publishes
  # as Latest. The script stops at the local tag and says what comes next.
  assert "it says how to push rather than pushing" \
    bash -c 'printf "%s" "$1" | grep -q "git push origin v5.5.5-rc1"' _ "$out"
  assert "tree clean" tree_clean "$D"
else
  fail "the bump-then-tag flow exited non-zero"
fi

echo "== case 12: a tag whose version the tree does not carry is refused =="
# The gate the manual flow never had. Cutting v9.9.9-rc1 from a tree still at
# 0.5.0 would build Stalagmight_0.5.0_* artifacts under a tag promising 9.9.9.
D=$(fresh_clone case12)
if out=$("$D/scripts/release-tag.sh" v9.9.9-rc1 2>&1); then
  fail "tagged a version the version files do not carry"
else
  pass "refused a tag the version files do not match"
  assert "message names the mismatch" \
    bash -c 'printf "%s" "$1" | grep -q "version files do not match the tag"' _ "$out"
  assert "message names the bump that would fix it" \
    bash -c 'printf "%s" "$1" | grep -q "release.sh 9.9.9"' _ "$out"
  refute "no tag created" has_tag "$D" v9.9.9-rc1
  assert "tree untouched" tree_clean "$D"
fi

echo "== case 12b: a drifted Cargo.lock is caught here too, not just at bump time =="
# The lock is the version file nothing fails on (#352): it is not built with
# --locked, so a wrong value is invisible until someone reads it. release.sh
# refuses to bump past a drifted lock; tagging has to refuse for the same reason,
# or the whole check is bypassed by taking the other flow.
D=$(fresh_clone case12b)
CUR=$(version_of "$D")
set_lock_version "$D" 0.0.1
git -C "$D" commit --quiet -am "desync Cargo.lock"
if out=$("$D/scripts/release-tag.sh" "v$CUR-rc1" 2>&1); then
  fail "tagged a tree whose Cargo.lock disagreed with the tag"
else
  pass "refused a drifted Cargo.lock"
  assert "message names the lock" \
    bash -c 'printf "%s" "$1" | grep -q "Cargo.lock records stalagmight at .0.0.1."' _ "$out"
  refute "no tag created" has_tag "$D" "v$CUR-rc1"
fi

echo "== case 12c: a missing version file refuses, rather than passing unread =="
# The check must fail CLOSED. `grep -c` on an unreadable file prints nothing, and
# `[ "" -ne 1 ]` is not false in bash - it is an error (status 2), which an `if`
# reads as false, skipping the refusal and returning success. Without the
# readability guard, version_files_match reports all four files agree having
# opened none of them. Each file gets its own case: the guard is per-file, and
# one shared assertion would pass on a fix that only covered the first.
for missing in package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock; do
  D=$(fresh_clone "case12c-$(basename "$missing")")
  CUR=$(version_of "$D")
  rm "$D/$missing"
  git -C "$D" commit --quiet -am "remove $missing"
  if out=$("$D/scripts/release-tag.sh" "v$CUR-rc1" 2>&1); then
    fail "tagged with $missing missing"
  else
    assert "refusal names $missing" \
      bash -c 'printf "%s" "$1" | grep -q "$2"' _ "$out" "$missing"
    refute "no tag created without $missing" has_tag "$D" "v$CUR-rc1"
  fi
done
# release.sh shares the same helper, so it must fail closed too - it is the
# script that would otherwise rewrite three files and commit.
D=$(fresh_clone case12c-bump)
rm "$D/src-tauri/Cargo.toml"
git -C "$D" commit --quiet -am "remove Cargo.toml"
if "$D/scripts/release.sh" 7.7.7 --no-edit >/dev/null 2>&1; then
  fail "release.sh bumped with Cargo.toml missing"
else
  pass "release.sh refuses a missing version file"
  refute "package.json not rewritten" grep -q '"version": "7\.7\.7"' "$D/package.json"
  refute "no tag created" has_tag "$D" v7.7.7
fi

echo "== case 13: an existing tag is refused rather than moved =="
D=$(fresh_clone case13)
CUR=$(version_of "$D")
git -C "$D" tag "v$CUR-rc1"
if out=$("$D/scripts/release-tag.sh" "v$CUR-rc1" 2>&1); then
  fail "accepted a tag that already exists"
else
  pass "refused an existing tag"
  assert "message says candidates are kept, not recycled" \
    bash -c 'printf "%s" "$1" | grep -q "Cut the next number"' _ "$out"
fi

echo "== case 14: a dirty tree is refused =="
# A tag names a commit, so uncommitted work is not in the release - including
# changelog edits, which would make the notes on disk differ from the notes that
# publish. release.sh tolerates a dirty CHANGELOG.md because it is about to
# commit it; here there is no commit, so the exemption would be a hole.
D=$(fresh_clone case14)
CUR=$(version_of "$D")
printf '\n- an uncommitted note\n' >> "$D/CHANGELOG.md"
if out=$("$D/scripts/release-tag.sh" "v$CUR-rc1" 2>&1); then
  fail "tagged a dirty tree"
else
  pass "refused a dirty tree"
  # Naming the offending path, not just "the tree is dirty": with several clones
  # of this repo open, which file is dirty is the whole question.
  assert "message lists the dirty file" \
    bash -c 'printf "%s" "$1" | grep -q "CHANGELOG.md"' _ "$out"
  refute "no tag created" has_tag "$D" "v$CUR-rc1"
fi
# An untracked file blocks too, and the advice has to work: plain `git stash`
# leaves '??' files behind, so a message saying only "stash" sends the operator
# in a circle.
D=$(fresh_clone case14b)
CUR=$(version_of "$D")
printf 'scratch\n' > "$D/notes-to-self.txt"
if out=$("$D/scripts/release-tag.sh" "v$CUR-rc1" 2>&1); then
  fail "tagged with an untracked file present"
else
  pass "refused an untracked file"
  assert "message names the untracked file" \
    bash -c 'printf "%s" "$1" | grep -q "notes-to-self.txt"' _ "$out"
  assert "message gives advice that actually works" \
    bash -c 'printf "%s" "$1" | grep -q -- "git stash -u"' _ "$out"
fi

echo "== case 15: entries left under [Unreleased] refuse the tag =="
# The #406 headline. Once '## [X.Y.Z]' exists - the state every candidate after
# the first is cut from - release-notes.sh stops reading [Unreleased], so entries
# left there ship in the binary and are named nowhere. It always warned; nothing
# called it before the tag existed.
D=$(fresh_clone case15)
CUR=$(version_of "$D")
printf '%s\n' \
  '# Changelog' '' \
  '## [Unreleased]' '' '- a late fix nobody folded in' '' \
  "## [$CUR] - 2026-01-01" '' '- the notes as written' \
  > "$D/CHANGELOG.md"
git -C "$D" commit --quiet -am "leave an entry unfolded"
if out=$("$D/scripts/release-tag.sh" "v$CUR-rc1" 2>&1); then
  fail "tagged with entries left under [Unreleased]"
else
  pass "refused entries left under [Unreleased]"
  assert "message says what would go unpublished" \
    bash -c 'printf "%s" "$1" | grep -q "will NOT be published"' _ "$out"
  assert "message says how to fix it" \
    bash -c 'printf "%s" "$1" | grep -q "Fold those entries"' _ "$out"
  assert "message names the override" \
    bash -c 'printf "%s" "$1" | grep -q -- "--allow-unannounced"' _ "$out"
  refute "no tag created" has_tag "$D" "v$CUR-rc1"
fi
# Shipping them unmentioned is a legitimate choice - a late fix nobody wants to
# announce - but it has to be said out loud.
if "$D/scripts/release-tag.sh" "v$CUR-rc1" --allow-unannounced >/dev/null 2>&1; then
  assert "--allow-unannounced tags anyway" has_tag "$D" "v$CUR-rc1"
else
  fail "--allow-unannounced did not override the refusal"
fi

echo "== case 15b: an inherited \$CHANGELOG cannot redirect the gate =="
# release-notes.sh honours ${CHANGELOG:-...} so a fixture can be pointed at it -
# which is how the cases below drive it. release-tag.sh must NOT inherit that:
# CI publishes from the repository's own CHANGELOG.md, so an exported override
# would gate on one file while a different one ships. Same unfolded-entry
# fixture as case 15, plus a clean decoy in the environment.
D=$(fresh_clone case15b)
CUR=$(version_of "$D")
printf '%s\n' \
  '# Changelog' '' \
  '## [Unreleased]' '' '- a late fix nobody folded in' '' \
  "## [$CUR] - 2026-01-01" '' '- the notes as written' \
  > "$D/CHANGELOG.md"
git -C "$D" commit --quiet -am "leave an entry unfolded"
printf '%s\n' '# Changelog' '' '## [Unreleased]' '' \
  "## [$CUR] - 2026-01-01" '' '- decoy notes, nothing unfolded' > "$WORK/decoy.md"
if CHANGELOG="$WORK/decoy.md" "$D/scripts/release-tag.sh" "v$CUR-rc1" >/dev/null 2>&1; then
  fail "an exported \$CHANGELOG redirected the gate and the tag was created"
else
  pass "the gate read the repository's changelog, not the override"
  refute "no tag created" has_tag "$D" "v$CUR-rc1"
fi

echo "== case 16: a malformed tag is refused =="
D=$(fresh_clone case16)
# Asserting the MESSAGE, not just a non-zero exit. These tags fail the version
# check too, so an exit-code-only assertion stays green with the shape check
# deleted - it would be testing that a malformed tag fails for some other reason.
for bad in "0.5.0" "v0.5" "v0.5.0.1" "release-1"; do
  assert "refused '$bad' as malformed" \
    bash -c '"$1" "$2" 2>&1 | grep -q "is not a release tag"' _ "$D/scripts/release-tag.sh" "$bad"
done
assert "tree untouched" tree_clean "$D"

echo "== case 17: --dry-run shows the body and creates no tag =="
D=$(fresh_clone case17)
CUR=$(version_of "$D")
if out=$("$D/scripts/release-tag.sh" "v$CUR-rc1" --dry-run 2>&1); then
  assert "the body is rendered" \
    bash -c 'printf "%s" "$1" | grep -q "fixture entry"' _ "$out"
  assert "it says it would tag" \
    bash -c 'printf "%s" "$1" | grep -q "\[dry-run\] Would tag"' _ "$out"
  refute "no tag created" has_tag "$D" "v$CUR-rc1"
  assert "tree untouched" tree_clean "$D"
else
  fail "--dry-run exited non-zero on a taggable tree"
fi

echo "== case 18: a changelog with no notes at all is refused =="
# release-notes.sh falls back to the bare string "Release vX.Y.Z" when it finds
# no section. That publishes an announcement whose entire content is the version
# number already in the title.
D=$(fresh_clone case18)
CUR=$(version_of "$D")
printf '# Changelog\n\n## [Unreleased]\n' > "$D/CHANGELOG.md"
git -C "$D" commit --quiet -am "empty the changelog"
if out=$("$D/scripts/release-tag.sh" "v$CUR" 2>&1); then
  fail "tagged a release with no notes"
else
  pass "refused a release with no notes"
  assert "message names the empty sections" \
    bash -c 'printf "%s" "$1" | grep -q "has no notes for"' _ "$out"
  refute "no tag created" has_tag "$D" "v$CUR"
fi

echo "== case 19: a final tag reports what it cannot check for you =="
# Two advisories, both notes rather than gates: a release cut without any
# candidate is legitimate, and so is publishing a section dated earlier than
# today - but both are invisible unless something says so, and the date is
# published verbatim.
D=$(fresh_clone case19)
CUR=$(version_of "$D")
printf '%s\n' \
  '# Changelog' '' '## [Unreleased]' '' \
  "## [$CUR] - 2026-01-01" '' '- the notes as written' \
  > "$D/CHANGELOG.md"
git -C "$D" commit --quiet -am "fold the changelog"
if out=$("$D/scripts/release-tag.sh" "v$CUR" --dry-run 2>&1); then
  assert "notes that no candidate was ever cut" \
    bash -c 'printf "%s" "$1" | grep -q "never cut as a candidate"' _ "$out"
  assert "notes the stale changelog date" \
    bash -c 'printf "%s" "$1" | grep -q "but today is"' _ "$out"
  assert "says it publishes as Latest" \
    bash -c 'printf "%s" "$1" | grep -q "published as Latest"' _ "$out"
  refute "a final tag gets no candidate banner" \
    bash -c 'printf "%s" "$1" | grep -q "Release candidate for"' _ "$out"
else
  fail "a foldable final tag was refused"
fi
# The same advisories must NOT fire on a candidate: it is a candidate, so asking
# why no candidate exists is noise, and the ship date is not settled yet.
D=$(fresh_clone case19b)
CUR=$(version_of "$D")
printf '%s\n' \
  '# Changelog' '' '## [Unreleased]' '' \
  "## [$CUR] - 2026-01-01" '' '- the notes as written' \
  > "$D/CHANGELOG.md"
git -C "$D" commit --quiet -am "fold the changelog"
if out=$("$D/scripts/release-tag.sh" "v$CUR-rc1" --dry-run 2>&1); then
  refute "a candidate is not nagged about candidates" \
    bash -c 'printf "%s" "$1" | grep -q "never cut as a candidate"' _ "$out"
  refute "a candidate is not nagged about the date" \
    bash -c 'printf "%s" "$1" | grep -q "but today is"' _ "$out"
  assert "a candidate says it is a pre-release" \
    bash -c 'printf "%s" "$1" | grep -q "pre-release"' _ "$out"
else
  fail "a candidate dry-run was refused"
fi

echo "== release-notes.sh =="
NOTES="$PROJECT_ROOT/scripts/release-notes.sh"
FIXTURE="$WORK/CHANGELOG.md"
printf '%s\n' \
  '# Changelog' '' \
  '## [Unreleased]' '' '- unreleased thing' '' \
  '## [0.4.0] - 2026-01-01' '' '- the 0.4.0 thing' '' \
  > "$FIXTURE"

# This fixture has BOTH a [0.4.0] section and a non-empty [Unreleased], which is
# the shape that triggers the unfolded-entries warning below. The cases here are
# about body selection, so they discard stderr; the warning gets its own cases.
#
# ⚠ Paths go to `bash -c` as POSITIONAL ARGS, never spliced into the quoted body.
# `$WORK` comes from `mktemp -d "${TMPDIR:-/tmp}/..."` and `$NOTES` from the
# checkout path, so a space or a shell metacharacter in either would word-split
# inside the eval and the case would silently exercise something else.
# A final tag takes its own section.
assert "final tag uses its version's section" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "the 0.4.0 thing"' _ "$FIXTURE" "$NOTES"
refute "final tag has no candidate banner" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "Release candidate"' _ "$FIXTURE" "$NOTES"

# A candidate takes the notes of the version it is a candidate for, not its own
# (nonexistent) section, and says clearly that it is a candidate.
assert "candidate uses the base version's section" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0-rc1 2>/dev/null | grep -q "the 0.4.0 thing"' _ "$FIXTURE" "$NOTES"
assert "candidate is labelled as one" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0-rc1 2>/dev/null | grep -q "Release candidate for 0.4.0"' _ "$FIXTURE" "$NOTES"

# Before release.sh moves it, a version has no section yet - fall back to
# [Unreleased] rather than publishing an empty body.
assert "unmoved version falls back to Unreleased" \
  bash -c 'CHANGELOG="$1" bash "$2" v9.9.9 | grep -q "unreleased thing"' _ "$FIXTURE" "$NOTES"

# Every release explains how to get past the first-launch block and how to check
# what was downloaded.
assert "installing footer present" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "SHA256SUMS"' _ "$FIXTURE" "$NOTES"
assert "apple silicon caveat present" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "Apple Silicon only"' _ "$FIXTURE" "$NOTES"

# The signing claim in this footer went stale once already: the very change that
# made the macOS app signable (#350) was about to publish notes still saying the
# builds were unsigned, because the fix updated the changelog and docs but not the
# script that generates what recipients read. Pin both directions, so a future
# signing change has to update this text deliberately rather than silently ship a
# contradiction.
refute "footer no longer claims the builds are unsigned" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "not code-signed"' _ "$FIXTURE" "$NOTES"
assert "footer says macOS is ad-hoc signed" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "ad-hoc signed"' _ "$FIXTURE" "$NOTES"
# The quarantine command is the one instruction that holds on every macOS version;
# the Control-click override was removed in macOS 15, so it cannot be the only path
# offered.
assert "footer gives the version-independent quarantine command" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "xattr -dr com.apple.quarantine"' _ "$FIXTURE" "$NOTES"

# --- Entries left under [Unreleased] after the section exists ---
# Between rc1 and rc2, four fixes sat under [Unreleased] while the tag read
# `## [0.5.0]`, so they shipped in the binary and appeared nowhere in the notes.
# Nothing failed; only reading the rendered body caught it.
assert "warns that unfolded [Unreleased] entries will not publish" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>&1 >/dev/null | grep -q "will NOT be published"' _ "$FIXTURE" "$NOTES"
assert "the warning counts the lines at risk" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>&1 >/dev/null | grep -q "^warning: 1 line(s)"' _ "$FIXTURE" "$NOTES"
assert "a candidate warns too, not just a final tag" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0-rc1 2>&1 >/dev/null | grep -q "will NOT be published"' _ "$FIXTURE" "$NOTES"

# The warning MUST NOT reach stdout: release.yml redirects stdout into the release
# body, so a warning printed there would be published to every recipient.
refute "the warning never contaminates the published body" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "will NOT be published"' _ "$FIXTURE" "$NOTES"

# Folded changelog: the section exists and [Unreleased] is empty, which is the
# state a correctly-prepared tag is cut from. Warning here would train people to
# ignore it.
printf '%s\n' \
  '# Changelog' '' \
  '## [Unreleased]' '' \
  '## [0.4.0] - 2026-01-01' '' '- the 0.4.0 thing' \
  > "$WORK/folded.md"
# Paired with the refute below on purpose: `release-notes.sh` is `set -euo pipefail`,
# so a missing or misnamed fixture makes it exit non-zero, print nothing, and the
# refute passes having tested nothing. This assertion is what proves the invocation
# actually ran.
assert "the folded fixture still renders a body" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "the 0.4.0 thing"' _ "$WORK/folded.md" "$NOTES"
refute "silent once the entries have been folded in" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>&1 >/dev/null | grep -q "will NOT be published"' _ "$WORK/folded.md" "$NOTES"

# A section holding only a heading has nothing to fold, so warning about it would
# be a false alarm - and a warning that fires on a good state is one people learn
# to ignore. Same pairing as above: prove the run happened before trusting silence.
printf '%s\n' \
  '# Changelog' '' \
  '## [Unreleased]' '' '### Fixed' '' \
  '## [0.4.0] - 2026-01-01' '' '- the 0.4.0 thing' \
  > "$WORK/heading-only.md"
assert "the heading-only fixture still renders a body" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>/dev/null | grep -q "the 0.4.0 thing"' _ "$WORK/heading-only.md" "$NOTES"
refute "silent when [Unreleased] holds only a heading" \
  bash -c 'CHANGELOG="$1" bash "$2" v0.4.0 2>&1 >/dev/null | grep -q "will NOT be published"' _ "$WORK/heading-only.md" "$NOTES"

# On the fallback path [Unreleased] IS the body, so there is nothing at risk.
refute "silent when [Unreleased] is what gets published" \
  bash -c 'CHANGELOG="$1" bash "$2" v9.9.9 2>&1 >/dev/null | grep -q "will NOT be published"' _ "$FIXTURE" "$NOTES"

# An empty changelog must still yield a usable body, never nothing.
printf '%s\n' '# Changelog' '' '## [Unreleased]' '' > "$WORK/empty.md"
assert "empty changelog still produces a body" \
  bash -c 'CHANGELOG='"$WORK"'/empty.md bash '"$NOTES"' v1.2.3 | grep -q "Release v1.2.3"'

echo "== case 20: a weekly-shaped tag is refused, not hand-tagged =="
D=$(fresh_clone case20)
CUR=$(version_of "$D")
if out=$("$D/scripts/release-tag.sh" "v${CUR}-weekly.20260817" 2>&1); then
  fail "tagged a weekly-shaped tag by hand"
else
  pass "refused a weekly-shaped tag"
  assert "message names weekly tags as reserved for the automated pipeline" \
    bash -c 'printf "%s" "$1" | grep -q "automated weekly pipeline"' _ "$out"
  refute "no tag created" has_tag "$D" "v${CUR}-weekly.20260817"
fi

# next_patch X.Y.Z -> X.Y.(Z+1), for fixturing a weekly tag with a base
# strictly above the version under test.
next_patch() {
  local maj min patch
  read -r maj min patch <<<"$(printf '%s' "$1" | tr . ' ')"
  printf '%s.%s.%s' "$maj" "$min" "$((patch + 1))"
}

echo "== case 21: a stable tag warns when a STRICTLY HIGHER weekly tag exists =="
D=$(fresh_clone case21)
CUR=$(version_of "$D")
NEXT=$(next_patch "$CUR")
git -C "$D" tag "v${NEXT}-weekly.20260817"
if out=$("$D/scripts/release-tag.sh" "v$CUR" --dry-run 2>&1); then
  assert "warns about the higher weekly tag" \
    bash -c 'printf "%s" "$1" | grep -q "weekly tag v'"$NEXT"'-weekly.20260817 already carries"' _ "$out"
  assert "explains weekly-track users will not be offered it" \
    bash -c 'printf "%s" "$1" | grep -q "will not be offered"' _ "$out"
else
  fail "a stable tag with a higher weekly tag was refused outright"
fi

echo "== case 21-equal: an EQUAL-base weekly tag does not warn (convergence, not supersession) =="
D=$(fresh_clone case21equal)
CUR=$(version_of "$D")
git -C "$D" tag "v${CUR}-weekly.20260817"
if out=$("$D/scripts/release-tag.sh" "v$CUR" --dry-run 2>&1); then
  refute "no supersession warning at an equal base" \
    bash -c 'printf "%s" "$1" | grep -q "already carries"' _ "$out"
else
  fail "a stable tag with only an equal-base weekly tag was refused outright"
fi

echo "== case 21b: a stable tag stays silent when no weekly tag supersedes it =="
D=$(fresh_clone case21b)
CUR=$(version_of "$D")
if out=$("$D/scripts/release-tag.sh" "v$CUR" --dry-run 2>&1); then
  refute "no supersession warning with no weekly tags at all" \
    bash -c 'printf "%s" "$1" | grep -q "already carries"' _ "$out"
else
  fail "a stable tag with no weekly tags was refused outright"
fi

echo "== case 21c: a lower weekly tag does not trigger the supersession warning =="
D=$(fresh_clone case21c)
CUR=$(version_of "$D")
git -C "$D" tag "v0.0.1-weekly.20260101"
if out=$("$D/scripts/release-tag.sh" "v$CUR" --dry-run 2>&1); then
  refute "no supersession warning from a lower weekly tag" \
    bash -c 'printf "%s" "$1" | grep -q "already carries"' _ "$out"
else
  fail "a stable tag with only a lower weekly tag was refused outright"
fi

echo "== case 22: the 9->10 boundary is compared numerically, not lexically =="
# All three pre-PR reviewers flagged this: a lexical `sort` (or a lexical
# comparison anywhere in the scan) puts "0.5.10" ahead of "0.5.9" for the
# wrong reason (a leading '1' sorts before '9'), which can either miss a
# genuinely higher weekly base or short-circuit on the wrong tag first.
# Pins the exact scenario: weekly base 0.5.10 exists, tagging stable v0.5.9
# must still warn.
D=$(fresh_clone case22)
if ! "$D/scripts/release.sh" 0.5.9 --no-edit --no-tag >/dev/null 2>&1; then
  fail "could not fixture the tree at 0.5.9 to set up case 22"
else
  git -C "$D" tag "v0.5.10-weekly.20260817"
  if out=$("$D/scripts/release-tag.sh" v0.5.9 --dry-run 2>&1); then
    assert "warns about the numerically (not lexically) higher weekly base" \
      bash -c 'printf "%s" "$1" | grep -q "weekly tag v0.5.10-weekly.20260817 already carries"' _ "$out"
  else
    fail "v0.5.9 with a v0.5.10-weekly tag present was refused outright"
  fi
fi

echo
echo "release tooling: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
