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
  # through app bundles. Build a *distinct* dev bundle (Peer-dev.app, bundle
  # id dev.aaronzhang.peer.dev, scheme peer-dev://) so a co-installed prod
  # /Applications/Peer.app — same identifier, same peer:// scheme — can't
  # win the LaunchServices lookup and steal our OAuth callback.
  APP_DIR="$(dirname "${BIN}")/Peer-dev.app"
  mkdir -p "${APP_DIR}/Contents/MacOS"
  cp "Info.plist" "${APP_DIR}/Contents/Info.plist"
  PLIST="${APP_DIR}/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier dev.aaronzhang.peer.dev" "${PLIST}" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Set :CFBundleName Peer-dev" "${PLIST}" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName Peer-dev" "${PLIST}" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Set :CFBundleURLTypes:0:CFBundleURLName com.aaronzhang.peer.dev.auth" "${PLIST}" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Set :CFBundleURLTypes:0:CFBundleURLSchemes:0 peer-dev" "${PLIST}" 2>/dev/null || true
  # Hardlink (not symlink) the binary into the bundle. With a symlink, exec'ing
  # via the bundle path resolves to target/debug/Peer at the kernel layer and
  # macOS treats the process as unbundled — so peer-dev:// events get routed to
  # a *new* Peer-dev.app instance instead of forwarded to the running one. The
  # new instance starts with PENDING_STATE empty (nonce check fails) and is
  # usually killed by single_instance before its on_open_url even fires.
  # APFS clonefile keeps this cheap; falls back to plain cp on other fs.
  rm -f "${APP_DIR}/Contents/MacOS/Peer"
  cp -c "${BIN}" "${APP_DIR}/Contents/MacOS/Peer" 2>/dev/null \
    || ln -f "${BIN}" "${APP_DIR}/Contents/MacOS/Peer" 2>/dev/null \
    || cp -p "${BIN}" "${APP_DIR}/Contents/MacOS/Peer"
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
    -f "${APP_DIR}" 2>/dev/null || true

  # Drop the legacy Peer.app dev bundle (pre-split) so it stops competing
  # for the peer:// scheme alongside any installed prod build.
  LEGACY_APP="$(dirname "${BIN}")/Peer.app"
  if [[ -d "${LEGACY_APP}" ]]; then
    /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister \
      -u "${LEGACY_APP}" 2>/dev/null || true
    rm -rf "${LEGACY_APP}"
  fi

  # Exec the bundled copy, not the raw binary. macOS associates a process
  # with an .app bundle by its executable path's parent .app dir — running
  # the raw binary makes the process unbundled from LaunchServices' POV,
  # so URL events for peer-dev:// can't be forwarded to it.
  exec "${APP_DIR}/Contents/MacOS/Peer" "$@"
fi
exec "${BIN}" "$@"
