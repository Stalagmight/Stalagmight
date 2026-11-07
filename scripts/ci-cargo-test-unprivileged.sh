#!/usr/bin/env bash
# Run ONE command (the CI `cargo test` step) with its Rust test binaries started
# as an unprivileged user (#576 NF-ROOT-PERM).
#
#   ./scripts/ci-cargo-test-unprivileged.sh cargo test --workspace ...
#
# Root - and any process holding CAP_DAC_OVERRIDE - ignores mode bits, so a
# test that takes a permission away (a read-only folder, a 0444 file, an
# unsearchable directory) cannot mean anything there. Those tests assert that
# the bits bite and fail otherwise (stalagmight_models::test_support), and the
# self-hosted runner is a container running as uid 0. On a runner that is
# already unprivileged (GitHub-hosted) this script just runs the command.
#
# Compilation stays with the job's user: cargo, the target dir, the registry
# and sccache are untouched, so the build cache works as before. Only the test
# binaries cross over, through Cargo's target runner: the script exports
# CARGO_TARGET_<HOST TRIPLE>_RUNNER for the command it runs and for nothing
# else. It used to append that variable to $GITHUB_ENV, which handed it to
# every later step of the job too, and `cargo run` honours the runner as well:
# `pnpm gen:ipc-types` then ran the generator as the test user, which cannot
# write src/types/ipc, and the check job failed on every root run (#576
# RV5-PI-1). Doctests are run by rustdoc rather than through the runner and
# stay with the job's user; none of them depends on permissions.
#
# The test user must be able to write what the tests write: their HOME (a
# fresh temp dir), the temp dir, XDG_CONFIG_HOME when the job sets it (pass
# the same value to this step), and src-tauri/bindings, which the ts-rs export
# tests regenerate.
#
# STALAGMIGHT_TEST_USER names an existing account to use instead of creating
# `stalagmight-test` (for a local dry run). A dry run of the whole step order
# runs this script as root, then `pnpm gen:ipc-types` with the shell's own
# environment: the generator must still run as root and write src/types/ipc.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <command> [args...]   (e.g. $0 cargo test --workspace ...)" >&2
  exit 2
fi

if [ "$(id -u)" -ne 0 ]; then
  echo "Already unprivileged ($(id -un)): the Rust tests run as this user."
  exec "$@"
fi
command -v setpriv >/dev/null || {
  echo "::error::setpriv (util-linux) is missing, so the Rust tests cannot drop root"
  exit 1
}

user=${STALAGMIGHT_TEST_USER:-stalagmight-test}
if ! id -u "$user" >/dev/null 2>&1; then
  useradd --system --user-group --no-create-home --shell /usr/sbin/nologin "$user"
fi
uid=$(id -u "$user")
gid=$(id -g "$user")
if [ "$uid" -eq 0 ]; then
  echo "::error::$user is root"
  exit 1
fi

repo=$(cd "$(dirname "$0")/.." && pwd)
home=$(mktemp -d)
chown "$uid:$gid" "$home"
chown -R "$uid:$gid" "$repo/src-tauri/bindings"
if [ -n "${XDG_CONFIG_HOME:-}" ]; then
  mkdir -p "$XDG_CONFIG_HOME"
  chown -R "$uid:$gid" "$XDG_CONFIG_HOME"
fi

runner=(setpriv "--reuid=$uid" "--regid=$gid" --clear-groups env "HOME=$home" "USER=$user" "LOGNAME=$user")

# Prove the drop before the tests rely on it: as the test user, a read-only
# directory it owns must refuse a new file.
probe=$(mktemp -d)
chown "$uid:$gid" "$probe"
chmod 0555 "$probe"
if "${runner[@]}" touch "$probe/probe" 2>/dev/null; then
  echo "::error::$user still writes a read-only directory: mode bits do not bite"
  exit 1
fi
chmod 0755 "$probe"
rm -rf "$probe"

host=$(rustc -vV | sed -n 's/^host: //p')
if [ -z "$host" ]; then
  echo "::error::cannot read the host triple from rustc -vV"
  exit 1
fi
var="CARGO_TARGET_$(printf '%s' "$host" | tr '[:lower:]-' '[:upper:]_')_RUNNER"
export "$var=${runner[*]}"
echo "The Rust test binaries of this command run as $user ($uid:$gid) through $var."
exec "$@"
