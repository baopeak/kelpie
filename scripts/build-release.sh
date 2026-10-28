#!/usr/bin/env bash
# Builds the Kelpie installer for the current platform:
#   Windows → NSIS installer (kelpied.exe, kelpie-stt.exe + sideloaded ConPTY next to Kelpie.exe)
#   macOS   → Kelpie.app + .dmg (kelpied, kelpie-stt inside Contents/MacOS)
# Usage: scripts/build-release.sh [extra tauri build args]
# Bumps the patch version first (0.1.0 → 0.1.1), so each installer has its own number.
set -euo pipefail
cd "$(dirname "$0")/.."

# Every build gets a new version: bump the patch number wherever the version is declared
# (workspace Cargo.toml, app/package.json, tauri.conf.json), keeping them in step.
# KELPIE_KEEP_VERSION=1 (the release workflow) builds the version the tag names instead.
if [ -n "${KELPIE_KEEP_VERSION:-}" ]; then
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
else
version=$(python - <<'EOF'
import re
files = {
    "Cargo.toml": r'(?m)^(version = ")(\d+)\.(\d+)\.(\d+)(")',
    "app/package.json": r'("version": ")(\d+)\.(\d+)\.(\d+)(")',
    "app/src-tauri/tauri.conf.json": r'("version": ")(\d+)\.(\d+)\.(\d+)(")',
}
current = set()
for path, pattern in files.items():
    m = re.search(pattern, open(path, encoding="utf-8", newline="").read())
    current.add(m.group(2, 3, 4))
assert len(current) == 1, f"versions differ: {current}"
major, minor, patch = current.pop()
new = f"{major}.{minor}.{int(patch) + 1}"
for path, pattern in files.items():
    # newline="" keeps each file's own line endings (CRLF or LF).
    text = open(path, encoding="utf-8", newline="").read()
    text = re.sub(pattern, lambda m: m.group(1) + new + m.group(5), text, count=1)
    open(path, "w", encoding="utf-8", newline="").write(text)
print(new)
EOF
)
fi
echo "Version: $version"

triple=$(rustc -vV | sed -n 's/^host: //p')
ext=""
case "$triple" in *windows*) ext=".exe" ;; esac

out_dir=$(cargo metadata --format-version 1 --no-deps | python -c "import json,sys;print(json.load(sys.stdin)['target_directory'])")
mkdir -p app/src-tauri/binaries
target_args=()
bundle_dir="$out_dir/release/bundle"
if [ "${KELPIE_TARGET:-}" = "universal-apple-darwin" ]; then
  # One macOS build for Apple Silicon and Intel: each sidecar for both, joined with lipo.
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release -p kelpied -p kelpie-stt --target "$t"
  done
  for bin in kelpied kelpie-stt; do
    lipo -create -output "app/src-tauri/binaries/$bin-universal-apple-darwin" \
      "$out_dir/aarch64-apple-darwin/release/$bin" "$out_dir/x86_64-apple-darwin/release/$bin"
  done
  target_args=(--target universal-apple-darwin)
  bundle_dir="$out_dir/universal-apple-darwin/release/bundle"
else
  cargo build --release -p kelpied -p kelpie-stt
  # Tauri sidecars are looked up as <name>-<target triple><ext>.
  cp "$out_dir/release/kelpied$ext" "app/src-tauri/binaries/kelpied-$triple$ext"
  cp "$out_dir/release/kelpie-stt$ext" "app/src-tauri/binaries/kelpie-stt-$triple$ext"
fi

cd app
pnpm install --frozen-lockfile
# (macOS ships bash 3.2: an empty array needs the ${a[@]+…} form under `set -u`.)
pnpm tauri build --config src-tauri/tauri.bundle.json ${target_args[@]+"${target_args[@]}"} "$@"
echo "Bundles: $bundle_dir"
