#!/bin/bash
# Build, sign, and serve an arm64 Android APK for sideloading.
# Usage: ./scripts/build-android.sh
#
# Serves the signed APK on port 9123. Download from your tablet at:
#   http://<server-ip>:9123/app-arm64-release.apk

set -euo pipefail

# Strip SQLite modules a hostile .stalx could reach (#608, docs/toolchain.md).
export LIBSQLITE3_FLAGS="-USQLITE_ENABLE_FTS3 -USQLITE_ENABLE_FTS3_PARENTHESIS -USQLITE_ENABLE_FTS5 -USQLITE_ENABLE_RTREE -USQLITE_ENABLE_DBSTAT_VTAB -DSQLITE_OMIT_LOAD_EXTENSION"

APK_DIR="src-tauri/gen/android/app/build/outputs/apk/universal/release"
KEYSTORE="$APK_DIR/debug.keystore"
UNSIGNED="$APK_DIR/app-universal-release-unsigned.apk"
SIGNED="$APK_DIR/app-arm64-release.apk"

echo "==> Building arm64 APK..."
pnpm tauri android build --target aarch64 --apk true

# Generate debug keystore if missing
if [ ! -f "$KEYSTORE" ]; then
  echo "==> Generating debug keystore..."
  keytool -genkeypair -v \
    -keystore "$KEYSTORE" \
    -alias androiddebugkey \
    -keyalg RSA -keysize 2048 -validity 10000 \
    -storepass android -keypass android \
    -dname "CN=Debug,O=Debug,C=US"
fi

echo "==> Signing APK..."
apksigner sign \
  --ks "$KEYSTORE" \
  --ks-key-alias androiddebugkey \
  --ks-pass pass:android \
  --key-pass pass:android \
  --out "$SIGNED" \
  "$UNSIGNED"

echo "==> Signed APK: $(ls -lh "$SIGNED" | awk '{print $5}')"

# Restart HTTP server
pkill -f "http.server 9123" 2>/dev/null || true
sleep 1
nohup python3 -m http.server 9123 --bind 0.0.0.0 --directory "$APK_DIR" > /dev/null 2>&1 &

echo "==> Serving at http://0.0.0.0:9123/app-arm64-release.apk"
