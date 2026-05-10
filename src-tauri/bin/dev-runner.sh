#!/usr/bin/env bash
# Cargo runner shim used by `pnpm tauri:dev` on macOS. Re-signs the freshly
# linked dev binary with a stable identifier (dev.aaronzhang.peer) and the
# entitlements file before exec'ing it. Without this, every `cargo build`
# produces a different linker-signed adhoc identity, which invalidates the
# user's Screen Recording / Microphone TCC grants on each rebuild.
set -euo pipefail
BIN="$1"; shift
# Only re-sign the actual Peer GUI binary. Test/bench binaries get hashed
# names like `peer_lib-ac6b3fd0020a4f53` and don't need (or want) the
# stable bundle identity — applying entitlements to them breaks `cargo test`.
#
# We deliberately do NOT pass --entitlements or --options runtime here:
#   - The release entitlements.plist claims restricted entitlements
#     (cs.allow-jit, cs.disable-library-validation, audio-input, etc.) which
#     macOS AMFI refuses to honor on adhoc-signed binaries — the kernel kills
#     the process at exec with "code signature validation failed" and no log
#     output. Even an entitlements file that claims only audio-input or
#     sandbox keys triggers this on Apple Silicon.
#   - The dev binary still gets mic/screen access via the standard TCC
#     prompts (driven by the NSMicrophoneUsageDescription / NSScreenCapture
#     UsageDescription strings embedded in Info.plist by build.rs), and
#     non-sandboxed dev apps don't need the device.* entitlements anyway.
# The stable identifier is what keeps TCC grants persisting across rebuilds.
if [[ "$(basename "${BIN}")" == "Peer" ]]; then
  if ! codesign --force --sign - \
       --identifier dev.aaronzhang.peer \
       -r='designated => identifier "dev.aaronzhang.peer"' \
       "${BIN}" 2>&1; then
    echo "dev-runner: codesign failed; TCC grants will not persist across rebuilds" >&2
  fi

  # `tauri dev` executes the raw binary, but macOS URL schemes are registered
  # through app bundles. Keep the debug bundle's plist in sync with our dev
  # plist so `peer://...` links launched from the browser resolve to Peer.
  APP_DIR="$(dirname "${BIN}")/Peer.app"
  if [[ -d "${APP_DIR}" ]]; then
    mkdir -p "${APP_DIR}/Contents/MacOS"
    cp "Info.plist" "${APP_DIR}/Contents/Info.plist"
    ln -sf "$(cd "$(dirname "${BIN}")" && pwd)/Peer" "${APP_DIR}/Contents/MacOS/Peer"
    /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
      -f "${APP_DIR}" 2>/dev/null || true
  fi
fi
exec "${BIN}" "$@"
