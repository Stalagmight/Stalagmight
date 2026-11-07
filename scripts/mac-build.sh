#!/bin/bash
# mac-build.sh - Build and test Stalagmight on macOS (standalone)
# Assumes development tools from mac-setup.sh are already installed.
set -euo pipefail

info()  { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
ok()    { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33m  ⚠ %s\033[0m\n' "$*"; }
die()   { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

# ── Parse arguments ──────────────────────────────────────────────────────────
PROJECT_DIR=""
TEST_UI=false
TEST_UI_HEADED=false
TEST_E2E=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --project-dir)
      PROJECT_DIR="$2"
      shift 2
      ;;
    --test-ui)
      TEST_UI=true
      shift
      ;;
    --test-ui-headed)
      TEST_UI_HEADED=true
      shift
      ;;
    --test-e2e)
      TEST_E2E=true
      shift
      ;;
    *)
      die "Unknown argument: $1"
      ;;
  esac
done

# Default to parent of this script's directory
if [[ -z "$PROJECT_DIR" ]]; then
  SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
  PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
fi

[[ -f "$PROJECT_DIR/package.json" ]] || die "Not a valid project directory: $PROJECT_DIR"

# Ask cargo where it actually writes rather than assuming src-tauri/target.
# CARGO_TARGET_DIR, $CARGO_HOME/config.toml or a .cargo/config.toml above the
# clone can redirect target-dir (this repo's machines put it on an external
# disk; per-machine settings stay out of the committed .cargo/config.toml, see
# docs/toolchain.md), and cargo resolves config files from the working
# directory, which is why this runs from inside PROJECT_DIR. Getting it wrong is
# silent: the signing and reporting steps below just find nothing.
CARGO_TARGET="$(cd "$PROJECT_DIR/src-tauri" && cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null)"
if [[ -z "$CARGO_TARGET" ]]; then
  warn "Could not read cargo's target directory; assuming src-tauri/target"
  CARGO_TARGET="$PROJECT_DIR/src-tauri/target"
fi

# ── Environment ──────────────────────────────────────────────────────────────
# Treat this as a CI build (suppresses -dev version suffix in telemetry)
export CI=true

# Load .env if present (contains TELEMETRY_ENDPOINT and other secrets)
if [[ -f "$PROJECT_DIR/.env" ]]; then
  # shellcheck disable=SC1091
  set -a; source "$PROJECT_DIR/.env"; set +a
  info "Loaded .env"
fi

# shellcheck disable=SC1091
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"

# Ensure Homebrew is on PATH
if [[ -x /opt/homebrew/bin/brew ]]; then
  eval "$(/opt/homebrew/bin/brew shellenv)"
elif [[ -x /usr/local/bin/brew ]]; then
  eval "$(/usr/local/bin/brew shellenv)"
elif [[ -x "$HOME/.homebrew/bin/brew" ]]; then
  eval "$("$HOME/.homebrew/bin/brew" shellenv)"
fi

# If the pinned node@<major> was installed via Homebrew, ensure its bin is on
# PATH. The major comes from .nvmrc, the same file CI selects node from, so this
# cannot drift behind a toolchain bump.
if command -v brew &>/dev/null; then
  # Leading component only, tolerating a `v` prefix and a full semver; stripping
  # every non-digit would turn "24.1.0" into "24100" and silently skip the PATH
  # override. Guarded on the file existing so a stray shell diagnostic cannot leak.
  NODE_MAJOR=""
  if [[ -f "$PROJECT_DIR/.nvmrc" ]]; then
    NODE_MAJOR="$(tr -d 'v \t\r\n' < "$PROJECT_DIR/.nvmrc" | cut -d. -f1)"
  fi
  if [[ "$NODE_MAJOR" =~ ^[0-9]+$ ]]; then
    NODE_PREFIX="$(brew --prefix "node@$NODE_MAJOR" 2>/dev/null || true)"
    if [[ -n "$NODE_PREFIX" && -d "$NODE_PREFIX/bin" ]]; then
      export PATH="$NODE_PREFIX/bin:$PATH"
    fi
  fi
fi

# Say something when the node that will actually build is not the pinned one.
# Every branch above can legitimately no-op (no brew, no node@<major> formula -
# note Homebrew only ships even majors, so an odd pin has no keg at all), and
# silently building a release artifact on the wrong runtime is worth a warning even
# though it is usually fine. Advisory: this script must still work for anyone
# managing node with nvm/asdf instead.
if [[ "${NODE_MAJOR:-}" =~ ^[0-9]+$ ]] && command -v node &>/dev/null; then
  ACTUAL_MAJOR="$(node --version | tr -d 'v' | cut -d. -f1)"
  if [[ "$ACTUAL_MAJOR" != "$NODE_MAJOR" ]]; then
    warn "node on PATH is v$ACTUAL_MAJOR but .nvmrc pins $NODE_MAJOR - building with v$ACTUAL_MAJOR"
  fi
fi

info "Building Stalagmight in $PROJECT_DIR"
echo "  Node.js:  $(node --version)"
echo "  pnpm:     $(pnpm --version)"
echo "  Rust:     $(rustc --version)"
echo ""

# ── Step 1: Install dependencies ────────────────────────────────────────────
info "Installing dependencies"
pnpm install --dir "$PROJECT_DIR"
ok "Dependencies installed"

# ── Step 2: TypeScript check ────────────────────────────────────────────────
info "Running TypeScript type check"
(cd "$PROJECT_DIR" && pnpm tsc --noEmit)
ok "TypeScript check passed"

# ── Step 3: Install Playwright browsers (optional) ────────────────────────
if [[ "$TEST_UI" == true || "$TEST_UI_HEADED" == true ]]; then
  info "Installing Playwright browsers (Chromium)"
  (cd "$PROJECT_DIR" && pnpm exec playwright install chromium)
  ok "Playwright browsers installed"
fi

# ── Step 4: Playwright UI tests (optional) ────────────────────────────────
if [[ "$TEST_UI_HEADED" == true ]]; then
  info "Running Playwright tests (headed)"
  (cd "$PROJECT_DIR" && pnpm test:ui:headed)
  ok "Playwright tests passed (headed)"
elif [[ "$TEST_UI" == true ]]; then
  info "Running Playwright tests (headless)"
  (cd "$PROJECT_DIR" && pnpm test:ui)
  ok "Playwright tests passed (headless)"
fi

# ── Step 5: Rust tests ──────────────────────────────────────────────────────
# macOS defaults to 256 open files which is too low for the test suite
ulimit -n 4096 2>/dev/null || ulimit -n 1024 2>/dev/null || true
info "Running Rust tests (ulimit -n $(ulimit -n))"
# Mirrors the CI command: --workspace is required (without it only the app
# package runs and the member crates under src-tauri/crates are skipped), and
# --features telemetry compiles the telemetry collector and its tests. The dummy endpoint
# just satisfies build.rs under that feature; a real one from .env wins. Run
# from inside PROJECT_DIR: cargo reads the bundled-SQLite build flags from its
# .cargo/config.toml, found from the working directory (docs/toolchain.md).
if (cd "$PROJECT_DIR" && TELEMETRY_ENDPOINT="${TELEMETRY_ENDPOINT:-http://localhost/dummy}" \
  cargo test --manifest-path src-tauri/Cargo.toml --workspace --features telemetry); then
  ok "Rust tests passed"
else
  warn "Some Rust tests failed (continuing with build)"
fi

# ── Step 5a: Build debug binary for E2E (optional) ────────────────────
if [[ "$TEST_E2E" == true ]]; then
  # Same CARGO_TARGET_DIR convention as the agent harness (e2e/lib/env.ts).
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cargo_build}"
  info "Building debug binary with e2e feature for the agent harness"
  info "  CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
  (cd "$PROJECT_DIR" && VITE_E2E=1 pnpm tauri build --debug --no-bundle -- --features e2e)
  ok "E2E debug binary built"
fi

# ── Step 5b: Run the agent-harness journeys + stress tier (optional) ──
# The WDIO suite this used to run was retired in #549; the journeys plus the
# fractal-stress scenario are the real-app equivalent (the old run included
# the large-survey stress spec, so keep that coverage in this preflight too).
if [[ "$TEST_E2E" == true ]]; then
  info "Running agent-harness journeys"
  (cd "$PROJECT_DIR" && pnpm agent:journeys)
  ok "Agent-harness journeys passed"
  info "Running fractal-stress (iter 4)"
  (cd "$PROJECT_DIR" && pnpm agent:scenario fractal-stress --iterations 4)
  ok "fractal-stress passed"
fi

# ── Step 6: Production build ────────────────────────────────────────────────
# CI=true (exported above) tells Tauri's DMG bundler to skip AppleScript
# (needs Finder GUI). Without it, bundle_dmg.sh hangs in headless/SSH sessions.
info "Building production macOS app"
if [[ -n "${TELEMETRY_ENDPOINT:-}" ]]; then
  info "  Telemetry enabled (endpoint set)"
  (cd "$PROJECT_DIR" && pnpm tauri build -- --features telemetry)
else
  warn "  Telemetry disabled (TELEMETRY_ENDPOINT not set - add to .env)"
  (cd "$PROJECT_DIR" && pnpm tauri build)
fi
ok "Production build complete"

# ── Step 6a: Verify the ad-hoc code signature ───────────────────────────────
# Tauri signs the app itself now, via bundle.macOS.signingIdentity in
# tauri.conf.json. That matters for ordering: signing has to happen before the
# DMG is built, and this step used to run after, so the .app on disk got signed
# while the DMG next to it kept an unsigned copy. Verifying is all that is left
# to do here, and it is the more useful job, because an invalid signature is not
# a warning the user can dismiss - macOS calls the app "damaged" and refuses to
# open it at all (#350).
# Checks the app inside the DMG, not the loose bundle beside it, via the same
# script the release workflow runs. Verifying the loose .app would pass on the
# exact bug this guards: signing used to happen after the DMG was built, so the
# loose copy was signed while the shipped one was not.
DMG_OUT="$CARGO_TARGET/release/bundle/dmg"
if [[ -d "$DMG_OUT" ]] && compgen -G "$DMG_OUT/*.dmg" >/dev/null; then
  info "Verifying the code signature of the app inside the DMG"
  # Hard failure, not a warning. An unopenable bundle is the one defect that is
  # invisible until a human tries to launch it, so a yellow line a maintainer can
  # scroll past while the script still exits 0 is not enough.
  if ! "$PROJECT_DIR/scripts/verify-macos-signature.sh" "$DMG_OUT"; then
    die "The app inside the DMG is NOT validly signed, so macOS will call it damaged. Check bundle.macOS.signingIdentity in src-tauri/tauri.conf.json"
  fi
  ok "Signature valid (not notarized: first launch still has to be allowed once)"
else
  warn "No DMG found in $DMG_OUT - skipping signature verification"
fi

# ── Step 7: Report artifacts ────────────────────────────────────────────────
BUNDLE_DIR="$CARGO_TARGET/release/bundle/macos"
info "Build artifacts in $BUNDLE_DIR:"
if [[ -d "$BUNDLE_DIR" ]]; then
  find "$BUNDLE_DIR" -maxdepth 1 -type f -o -type d -maxdepth 1 | while read -r f; do
    if [[ "$f" != "$BUNDLE_DIR" ]]; then
      SIZE=$(du -sh "$f" | cut -f1)
      echo "  $SIZE  $(basename "$f")"
    fi
  done
else
  echo "  (bundle directory not found - check build output above)"
fi

# Also check for DMG
DMG_DIR="$CARGO_TARGET/release/bundle/dmg"
if [[ -d "$DMG_DIR" ]]; then
  info "DMG artifacts in $DMG_DIR:"
  find "$DMG_DIR" -maxdepth 1 -type f | while read -r f; do
    SIZE=$(du -sh "$f" | cut -f1)
    echo "  $SIZE  $(basename "$f")"
  done
fi

ok "macOS build complete!"
