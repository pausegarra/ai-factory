#!/usr/bin/env bash
set -euo pipefail

# AppImage's AI\x02 marker occupies ELF identification padding. Rosetta's
# loader rejects it; normalizing that padding leaves executable code intact.
# This affects the downloaded build helper, never the produced application.
[[ "${AI_FACTORY_DOCKER_ROSETTA:-0}" == 1 ]] || exit 1
factory_plugin=${1:-${XDG_CACHE_HOME:-$HOME/.cache}/tauri/linuxdeploy-plugin-appimage.AppImage}
[[ -f "$factory_plugin" ]] || exit 1
factory_signature=$(od -An -tx1 -N4 "$factory_plugin" | tr -d '[:space:]')
[[ "$factory_signature" == 7f454c46 ]] || exit 1
factory_magic=$(od -An -tx1 -j8 -N3 "$factory_plugin" | tr -d '[:space:]')
if [[ "$factory_magic" == 414902 ]]; then
  printf '\0\0\0' | dd of="$factory_plugin" bs=1 seek=8 conv=notrunc status=none
elif [[ "$factory_magic" != 000000 ]]; then
  exit 1
fi
APPIMAGE_EXTRACT_AND_RUN=1 "$factory_plugin" --plugin-api-version
