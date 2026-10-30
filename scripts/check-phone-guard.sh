#!/usr/bin/env bash
# Proves the release guard: with KELPIE_REQUIRE_PHONE=1 the daemon does not build without the phone client
# (crates/kelpied/build.rs). The build script is pointed at an empty folder, so app/dist-phone is left alone.
# Usage: scripts/check-phone-guard.sh   (run from anywhere; the daemon's dependencies must already be built to be quick)
set -euo pipefail
cd "$(dirname "$0")/.."
empty=$(mktemp -d)
log=$(mktemp)
trap 'rm -rf "$empty" "$log"' EXIT
if KELPIE_REQUIRE_PHONE=1 KELPIE_PHONE_DIST="$empty" cargo build -p kelpied >"$log" 2>&1; then
  echo "kelpied was built without the phone client although KELPIE_REQUIRE_PHONE=1"
  exit 1
fi
if ! grep -q 'pnpm build:phone' "$log"; then
  echo "the build failed, but not because the phone client is missing:"
  tail -20 "$log"
  exit 1
fi
echo "ok: KELPIE_REQUIRE_PHONE=1 refuses to build kelpied without the phone client"
