#!/usr/bin/env bash
# Coverage-guided fuzzing of the survey-file importers and the .stalx open path
# (cargo-fuzz / libFuzzer). Needs a nightly toolchain and cargo-fuzz:
#   rustup toolchain install nightly --profile minimal
#   cargo +nightly install cargo-fuzz --locked
# See docs/testing-recipes.md, "Parser fuzzing".
#
# Usage: src-tauri/fuzz/run.sh <command> [args]
#   list                         print the target names
#   build                        build every target (instrumented, overflow checks on)
#   seeds <target> [count]       (re)write <target>'s synthetic seed corpus
#   fuzz <target> [seconds]      fuzz <target> (default 600 s); seeds an empty corpus first
#   repro <target> <file>        run one input (a crash artifact) through <target>
#   tmin <target> <file>         minimise a crashing input
# Extra libFuzzer flags go after `--`, e.g. `run.sh fuzz raw_tml 60 -- -dict=x`.
#
# Env:
#   FUZZ_WORKDIR    corpora, artifacts and logs (default src-tauri/fuzz/work;
#                   the farm points it outside the checkout so corpora persist
#                   across builds)
#   FUZZ_FORKS      parallel libFuzzer workers (-fork), default 1
#   FUZZ_TIMEOUT    per-input time limit in seconds, default 10 (a slower input
#                   is reported as a hang)
#   FUZZ_RSS_MB     memory limit per worker, default 2048 (beyond it: an OOM finding)
#   FUZZ_MAX_LEN    largest input in bytes; default 16384 for the generator
#                   targets (decision bytes: a few KB already render files of
#                   thousands of shots), 262144 for raw_*, 1048576 for stalx_raw
#   FUZZ_SEEDS      seed count for an empty corpus, default 64
#   FUZZ_TOOLCHAIN  rustup toolchain, default nightly
#   FUZZ_TARGET_DIR cargo target dir, default src-tauri/fuzz/target
#   FUZZ_ARTIFACT_DIR, FUZZ_LOG_DIR  override where one fuzz run puts its
#                   artifacts and its log (default under FUZZ_WORKDIR)
# `fuzz` prints `FUZZ-FINDING <target> <crash|timeout|oom|...> <file>` per new
# artifact and exits 1 if there was any, else prints `FUZZ-CLEAN` and exits 0.
#
# Seeds are generated in code (never survey files) and corpora are never
# committed. A crash artifact replays on stable too:
#   STALAGMIGHT_FUZZ_REPLAY=<target>:<file> cargo test -p stalagmight-fuzz-gen --test fixed_seeds replay
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
TC=${FUZZ_TOOLCHAIN:-nightly}
WORK=${FUZZ_WORKDIR:-$HERE/work}
TD=${FUZZ_TARGET_DIR:-$HERE/target}
FORKS=${FUZZ_FORKS:-1}
TIMEOUT=${FUZZ_TIMEOUT:-10}
RSS=${FUZZ_RSS_MB:-2048}
MAXLEN=${FUZZ_MAX_LEN:-}
NSEEDS=${FUZZ_SEEDS:-64}

targets() {
  sed -n 's#^path = "fuzz_targets/\([a-z_]*\)\.rs"#\1#p' "$HERE/Cargo.toml"
}

need_target() {
  if ! targets | grep -qx "${1:-}"; then
    echo "unknown target '${1:-}'; known: $(targets | tr '\n' ' ')" >&2
    exit 2
  fi
}

bin_dir() {
  echo "$TD/$(rustc "+$TC" -vV | sed -n 's/^host: //p')/release"
}

build() {
  # Start from the app's lockfile so the fuzzed dependency versions are the
  # shipped ones; cargo only adds libFuzzer on top.
  cp "$REPO/src-tauri/Cargo.lock" "$HERE/Cargo.lock"
  cargo "+$TC" fuzz build --fuzz-dir "$HERE" --target-dir "$TD" -O -a -s none
}

seeds() {
  local t=$1 n=${2:-$NSEEDS}
  need_target "$t"
  mkdir -p "$WORK/corpus/$t"
  cargo run --quiet --manifest-path "$REPO/src-tauri/Cargo.toml" -p stalagmight-fuzz-gen \
    --example fuzz_seeds -- "$t" "$WORK/corpus/$t" "$n"
}

ensure_built() {
  local t=$1
  [ -x "$(bin_dir)/$t" ] || build
}

cmd=${1:-}
shift || true
extra=()
args=()
while [ $# -gt 0 ]; do
  if [ "$1" = "--" ]; then shift; extra=("$@"); break; fi
  args+=("$1"); shift
done

case "$cmd" in
  list) targets ;;
  build) build ;;
  seeds) seeds "${args[0]:-}" "${args[1]:-$NSEEDS}" ;;
  fuzz)
    t=${args[0]:-}; secs=${args[1]:-600}
    need_target "$t"
    ensure_built "$t"
    if [ -z "$MAXLEN" ]; then
      case "$t" in
        raw_*) MAXLEN=262144 ;;
        stalx_raw) MAXLEN=1048576 ;;
        *) MAXLEN=16384 ;;
      esac
    fi
    arts=${FUZZ_ARTIFACT_DIR:-$WORK/artifacts/$t}
    logs=${FUZZ_LOG_DIR:-$WORK/logs}
    mkdir -p "$WORK/corpus/$t" "$arts" "$logs"
    if [ -z "$(ls -A "$WORK/corpus/$t")" ]; then seeds "$t" "$NSEEDS"; fi
    log="$logs/$t-$(date +%Y%m%dT%H%M%S).log"
    before=$(mktemp)
    find "$arts" -type f | sort > "$before"
    echo "fuzzing $t for ${secs}s with $FORKS worker(s); log $log" >&2
    # Fork mode keeps going after a crash, hang or OOM and keeps every artifact.
    (cd "$logs" && "$(bin_dir)/$t" -fork="$FORKS" -ignore_crashes=1 -ignore_timeouts=1 \
      -ignore_ooms=1 -max_total_time="$secs" -timeout="$TIMEOUT" -rss_limit_mb="$RSS" \
      -max_len="$MAXLEN" -artifact_prefix="$arts/" -print_final_stats=1 \
      "${extra[@]}" "$WORK/corpus/$t") > "$log" 2>&1 || true
    tail -n 3 "$log"
    # One line per new artifact (crash-/timeout-/oom-<sha1>), plus the first
    # panic location in the log, so a scheduler can tell findings apart.
    new=$(find "$arts" -type f | sort | comm -13 "$before" -)
    rm -f "$before"
    if [ -n "$new" ]; then
      grep -a -m 20 -E "panicked at |ERROR: libFuzzer: (timeout|out-of-memory|deadly signal)" "$log" | sort | uniq -c | sort -rn | head -n 10 >&2 || true
      for f in $new; do
        echo "FUZZ-FINDING $t $(basename "$f" | cut -d- -f1) $f"
      done
      exit 1
    fi
    echo "FUZZ-CLEAN $t ${secs}s"
    ;;
  repro)
    t=${args[0]:-}; f=${args[1]:?file}
    need_target "$t"; ensure_built "$t"
    "$(bin_dir)/$t" -timeout="$TIMEOUT" -rss_limit_mb="$RSS" "${extra[@]}" "$f"
    ;;
  tmin)
    t=${args[0]:-}; f=${args[1]:?file}
    need_target "$t"; ensure_built "$t"
    "$(bin_dir)/$t" -minimize_crash=1 -runs=20000 -timeout="$TIMEOUT" -rss_limit_mb="$RSS" \
      -exact_artifact_path="$f.min" "${extra[@]}" "$f"
    ;;
  *)
    sed -n '2,/^set /{/^#/p}' "$0" >&2
    exit 2
    ;;
esac
