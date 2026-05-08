#!/usr/bin/env bash
set -euo pipefail

APP_PATH="src-tauri/target/release/bundle/macos/Peer.app"
DMG_DIR="src-tauri/target/release/bundle/dmg"
VERSION="$(node -p "require('./package.json').version")"
ARCH="$(uname -m)"
DMG_PATH="${DMG_DIR}/Peer_${VERSION}_${ARCH}.dmg"
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
TEAM_ID="${APPLE_TEAM_ID:-}"
NOTARY_PROFILE="${APPLE_NOTARYTOOL_PROFILE:-}"

if [[ -z "${IDENTITY}" ]]; then
  echo "APPLE_SIGNING_IDENTITY must be set to a Developer ID Application identity." >&2
  exit 1
fi

pnpm sidecar
pnpm tauri build --bundles app

codesign --force --deep --options runtime \
  --timestamp \
  --sign "${IDENTITY}" \
  --entitlements src-tauri/entitlements.plist \
  "${APP_PATH}"

mkdir -p "${DMG_DIR}"
rm -f "${DMG_PATH}"
hdiutil create -volname "Peer" \
  -srcfolder "${APP_PATH}" \
  -ov \
  -format UDZO \
  "${DMG_PATH}"

codesign --force --timestamp --sign "${IDENTITY}" "${DMG_PATH}"

if [[ -n "${NOTARY_PROFILE}" ]]; then
  notary_args=(--keychain-profile "${NOTARY_PROFILE}")
  if [[ -n "${TEAM_ID}" ]]; then
    notary_args+=(--team-id "${TEAM_ID}")
  fi
  xcrun notarytool submit "${DMG_PATH}" \
    "${notary_args[@]}" \
    --wait
  xcrun stapler staple "${DMG_PATH}"
else
  echo "APPLE_NOTARYTOOL_PROFILE is not set; skipping notarization." >&2
fi
