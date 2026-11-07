#!/bin/bash
# Helper script to start/restart the Tauri dev server

# Kill existing processes if they exist
pgrep -f "stalagmight" > /dev/null && pkill -f "stalagmight"
pgrep -f "vite.*1420" > /dev/null && pkill -f "vite.*1420"

sleep 1

# Load local env (.env is gitignored; see .env.example). Devs put TELEMETRY_ENDPOINT
# here to dogfood telemetry - it is intentionally NOT hardcoded in this script.
if [ -f .env ]; then
  set -a
  source ./.env
  set +a
fi

# Start the dev server
export VITE_IPC_TRACE=true

# Telemetry is on by default in dev when an endpoint is configured (in .env):
# compile the feature and force the runtime opt-in via STALAGMIGHT_TELEMETRY_ENABLED
# (the Settings toggle can still turn it off; dev sessions are tagged
# "<version>-dev-<hash>" so they're easy to exclude). Without TELEMETRY_ENDPOINT we
# build plain so dev still works with no .env. To compile telemetry but leave it
# off, put STALAGMIGHT_TELEMETRY_ENABLED=0 in .env.
if [ -n "$TELEMETRY_ENDPOINT" ]; then
  export STALAGMIGHT_TELEMETRY_ENABLED="${STALAGMIGHT_TELEMETRY_ENABLED:-1}"
  exec pnpm tauri dev --features telemetry
else
  exec pnpm tauri dev
fi
