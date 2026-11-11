#!/usr/bin/env bash
set -euo pipefail

# Run inside the Ubuntu build image with the repository mounted at /source.
tar -C /source \
  --exclude=.git --exclude=node_modules --exclude=target --exclude=dist \
  --exclude='*.sqlite3*' --exclude='.env*' -cf - . | tar -xf - -C /work
cd /work
npm ci
npm --prefix desktop ci
npm --prefix desktop test
cargo test --workspace --locked
factory_build_log=$(mktemp)
trap 'rm -f "$factory_build_log"' EXIT
if APPIMAGE_EXTRACT_AND_RUN=1 npm run build -- --bundles deb,appimage 2>&1 | tee "$factory_build_log"; then
  exit 0
fi
# Only retry packaging after a confirmed linuxdeploy failure, so a compiler
# failure can never be hidden by packaging an older executable.
if grep -q 'failed to run .*linuxdeploy' "$factory_build_log" && bash .ci/prepare-rosetta-appimage.sh; then
  APPIMAGE_EXTRACT_AND_RUN=1 npm run tauri -- bundle --bundles deb,appimage
else
  exit 1
fi
