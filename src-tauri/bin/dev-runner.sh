#!/usr/bin/env bash
# Cargo runner shim used by `pnpm tauri:dev` on macOS. Re-signs the freshly
# linked dev binary with a stable identifier (dev.aaronzhang.peer) and the
# entitlements file before exec'ing it. Without this, every `cargo build`
# produces a different linker-signed adhoc identity, which invalidates the
# user's Screen Recording / Microphone TCC grants on each rebuild.
set -euo pipefail
BIN="$1"; shift
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ENTITLEMENTS="${SCRIPT_DIR}/../entitlements.plist"
# Only re-sign the actual Peer GUI binary. Test/bench binaries get hashed
# names like `peer_lib-ac6b3fd0020a4f53` and don't need (or want) the
# stable bundle identity — applying entitlements to them breaks `cargo test`.
if [[ "$(basename "${BIN}")" == "Peer" ]]; then
  if ! codesign --force --sign - \
       --identifier dev.aaronzhang.peer \
       --entitlements "${ENTITLEMENTS}" \
       --options runtime \
       "${BIN}" 2>&1; then
    echo "dev-runner: codesign failed; TCC grants will not persist across rebuilds" >&2
  fi
fi
exec "${BIN}" "$@"
