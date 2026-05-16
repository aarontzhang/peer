#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS smoke tests only run on Darwin." >&2
  exit 1
fi

if [[ "${PEER_RUN_MAC_SMOKE:-}" != "1" ]]; then
  cat >&2 <<'MSG'
Set PEER_RUN_MAC_SMOKE=1 to run the opt-in desktop smoke test.
This launches the Tauri app and may require Screen Recording, Microphone, and Accessibility permissions.
MSG
  exit 0
fi

pnpm sidecar
pnpm tauri dev
