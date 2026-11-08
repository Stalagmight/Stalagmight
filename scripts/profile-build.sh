#!/usr/bin/env bash
#
# Profile cargo + tsc + vite build steps and write a timed log.
#
#   ./scripts/profile-build.sh              # warm runs only (fast, ~1-2 min)
#   ./scripts/profile-build.sh --cold       # cargo clean first (cold build; very slow)
#   ./scripts/profile-build.sh --release    # also time `cargo build --release` (slow)
#   ./scripts/profile-build.sh --vite       # also time `pnpm build` (vite cold build)
#   ./scripts/profile-build.sh --all        # everything (cold + release + vite)
#
# Run from the repo root. Writes results to ./build-profile-<timestamp>.log.
# Cargo's --timings flag also produces an HTML breakdown at
# <target>/cargo-timings/cargo-timing.html - open that for a per-crate view.

set -euo pipefail

cd "$(dirname "$0")/.."

# Strip SQLite modules a hostile .stalx could reach (#608, docs/toolchain.md).
export LIBSQLITE3_FLAGS="-USQLITE_ENABLE_FTS3 -USQLITE_ENABLE_FTS3_PARENTHESIS -USQLITE_ENABLE_FTS5 -USQLITE_ENABLE_RTREE -USQLITE_ENABLE_DBSTAT_VTAB -DSQLITE_OMIT_LOAD_EXTENSION"

COLD=0
RELEASE=0
VITE=0
for arg in "$@"; do
  case "$arg" in
    --cold)    COLD=1 ;;
    --release) RELEASE=1 ;;
    --vite)    VITE=1 ;;
    --all)     COLD=1; RELEASE=1; VITE=1 ;;
    -h|--help)
      sed -n '3,12p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $arg" >&2; exit 1 ;;
  esac
done

STAMP=$(date +%Y%m%d-%H%M%S)
LOG="build-profile-${STAMP}.log"
exec > >(tee "$LOG") 2>&1

echo "Logging to $LOG"
echo "Started: $(date)"
echo

###############################################################################
# Environment snapshot
###############################################################################

echo "=== System ==="
uname -a
if [[ "$OSTYPE" == darwin* ]]; then
  sysctl -n machdep.cpu.brand_string 2>/dev/null || true
  echo "physical CPUs: $(sysctl -n hw.physicalcpu)"
  echo "logical CPUs:  $(sysctl -n hw.logicalcpu)"
  echo "RAM:           $(( $(sysctl -n hw.memsize) / 1024 / 1024 / 1024 )) GB"
  echo "macOS:         $(sw_vers -productVersion 2>/dev/null || echo unknown)"
  echo "Xcode:         $(xcodebuild -version 2>/dev/null | head -1 || echo 'not installed')"
else
  grep "model name" /proc/cpuinfo | head -1 || true
  echo "logical CPUs: $(nproc)"
  free -h | head -2 || true
fi
echo

echo "=== Versions ==="
rustc --version 2>/dev/null || echo "rustc: not found"
cargo --version 2>/dev/null || echo "cargo: not found"
node  --version 2>/dev/null || echo "node:  not found"
pnpm  --version 2>/dev/null || echo "pnpm:  not found"
if command -v mold >/dev/null 2>&1; then
  mold --version | head -1
fi
echo

TARGET_DIR="${CARGO_TARGET_DIR:-src-tauri/target}"
echo "=== Cargo target dir ==="
echo "Path: $TARGET_DIR"
du -sh "$TARGET_DIR" 2>/dev/null || echo "(empty/missing)"
echo

###############################################################################
# Helpers
###############################################################################

# Tracks whether any profiled step exited non-zero, so the script can finish
# all steps (we still want later timings even if an early build fails) yet
# return a failing status to the caller/CI.
FAILED=0

# Run a command, prefix output with [label], print wall-clock time.
# Usage: run_timed "label" cmd args...
run_timed() {
  local label="$1"; shift
  echo "=== $label ==="
  echo "+ $*"
  local start end elapsed status
  start=$(date +%s)
  # An `if` condition is exempt from `set -e`, so a failed step is recorded
  # rather than aborting the run. pipefail makes the status reflect "$@",
  # not the trailing `tail`.
  if "$@" 2>&1 | tail -30; then
    status=0
  else
    status=$?
    echo "(command exited non-zero [$status] - see full log above)"
    FAILED=1
  fi
  end=$(date +%s)
  elapsed=$((end - start))
  printf '[wall-clock: %dm %02ds]\n\n' $((elapsed / 60)) $((elapsed % 60))
}

###############################################################################
# Optional: clean for a true cold measurement
###############################################################################

if [[ "$COLD" -eq 1 ]]; then
  echo "=== cargo clean ==="
  cargo clean --manifest-path src-tauri/Cargo.toml
  echo "(cleaned)"
  echo
fi

###############################################################################
# Cargo measurements
###############################################################################

# Snapshot current cache state so the user can tell which run is "cold-ish".
echo "=== Target dir size before checks ==="
du -sh "$TARGET_DIR" 2>/dev/null || echo "(empty)"
echo

# First check: state-dependent. If --cold this is the full cold build;
# otherwise it's whatever cargo decides needs rebuilding.
run_timed "cargo check (initial)" \
  cargo check --manifest-path src-tauri/Cargo.toml

# Touch lib.rs and re-check to measure the steady-state incremental cost
# of a single-file edit.
touch src-tauri/src/lib.rs
run_timed "cargo check (warm incremental, touch lib.rs)" \
  cargo check --manifest-path src-tauri/Cargo.toml

# cargo build with --timings produces an HTML report broken down per crate
# (slowest deps, parallelism, codegen time). Use the report to find what's
# worth removing or feature-trimming. We touch first so cargo actually does
# work even if check is already up to date.
touch src-tauri/src/lib.rs
run_timed "cargo build --timings (dev profile)" \
  cargo build --timings --manifest-path src-tauri/Cargo.toml

echo "[Timings report: $TARGET_DIR/cargo-timings/cargo-timing.html]"
echo

if [[ "$RELEASE" -eq 1 ]]; then
  run_timed "cargo build --release --timings" \
    cargo build --release --timings --manifest-path src-tauri/Cargo.toml
fi

###############################################################################
# TypeScript / Vite
###############################################################################

# First tsc run produces the .tsbuildinfo file (incremental cache).
run_timed "tsc --noEmit (first run)" pnpm typecheck

# Second run benefits from the cache - this is the true edit-loop cost.
run_timed "tsc --noEmit (incremental)" pnpm typecheck

if [[ "$VITE" -eq 1 ]]; then
  run_timed "vite build (frontend cold)" pnpm build
fi

###############################################################################
# Summary
###############################################################################

echo "=== Target dir size after ==="
du -sh "$TARGET_DIR" 2>/dev/null || echo "(empty)"
echo
echo "Finished: $(date)"
echo "Full log: $LOG"

if [[ "$FAILED" -ne 0 ]]; then
  echo "One or more profiled steps exited non-zero - see the log above." >&2
  exit 1
fi
